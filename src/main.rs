mod case;
mod logging;
mod osp;
mod snapshot;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};

use case::Case;
use osp::{Frame, Function, Operator, audio, product_info, settings, status};
use snapshot::Snapshot;

/// Control Ozlo Sleepbuds from a Linux desktop.
#[derive(Parser)]
struct Cli {
    /// Echo the log to stderr. Repeat for more detail.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print everything known about the case.
    Status,
    /// Print the charge of the case and each bud.
    Battery,
    /// List the sounds stored on the buds.
    Sounds,
    /// Show the alarms set on the case.
    Alarms,
    /// Query every readable function and print the raw and decoded replies.
    Dump,
    /// Send one Get frame given as spaced hex, such as `02 02 01 00`.
    Raw {
        /// The frame, without the BLE prefix byte.
        hex: Vec<String>,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let cli = Cli::parse();
    if let Err(error) = logging::start(cli.verbose) {
        eprintln!("warning: logging is off: {error:#}");
    }
    if let Err(error) = run(cli.command).await {
        log::error!("{error:?}");
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

async fn run(command: Command) -> anyhow::Result<()> {
    log::info!("ozzzlo {} started", env!("CARGO_PKG_VERSION"));
    let case = Case::connect().await.context("connecting to the case")?;
    match command {
        Command::Status => println!("{}", survey(&case).await?),
        Command::Battery => {
            let mut snapshot = Snapshot::default();
            case.exchange(&Frame::get(status::BATTERY_LEVEL), |frame| {
                snapshot.apply(frame);
            })
            .await
            .context("reading the battery level")?;
            let level = snapshot.battery.context("the battery reply was malformed")?;
            println!(
                "case {}%  left {}%  right {}%",
                level.case, level.left, level.right
            );
        }
        Command::Sounds => {
            let snapshot = survey(&case).await?;
            let current = snapshot.playback.map(|playback| playback.loop_id);
            for sound in &snapshot.sound_loops {
                let marker = if current == Some(sound.id) { "*" } else { " " };
                println!("{marker} {:3}  {}", sound.id, snapshot.loop_name(sound.id));
            }
        }
        Command::Alarms => {
            let snapshot = survey(&case).await?;
            for alarm in &snapshot.alarms {
                println!(
                    "{:2}  {:02}:{:02}  {:3}  {:<12} {:<16} volume {}",
                    alarm.index,
                    alarm.hour,
                    alarm.minute,
                    if alarm.enabled { "on" } else { "off" },
                    alarm.days.to_string(),
                    snapshot.alarm_sound_name(alarm.sound),
                    alarm.volume
                );
            }
        }
        Command::Dump => dump(&case).await?,
        Command::Raw { hex } => {
            let request: Frame = hex.join(" ").parse().context("parsing the frame")?;
            if request.operator != Operator::Get {
                bail!("raw sends only Get frames (operator 01)");
            }
            print_exchange(&case, &request).await?;
        }
    }
    Ok(())
}

/// Asks the case for its full state.
async fn survey(case: &Case) -> anyhow::Result<Snapshot> {
    let mut snapshot = Snapshot::default();
    let requests = [
        Frame::start(product_info::GET_ALL),
        Frame::start(settings::GET_ALL),
        Frame::start(status::GET_ALL),
        Frame::get(audio::NOW_PLAYING),
        Frame::get(audio::SOUND_LOOP_VOLUME),
    ];
    for request in requests {
        case.exchange(&request, |frame| snapshot.apply(frame))
            .await
            .with_context(|| format!("reading {}", request.function))?;
    }
    Ok(snapshot)
}

async fn dump(case: &Case) -> anyhow::Result<()> {
    let get = |block, id| Frame::get(Function { block, id });
    let requests = [
        Frame::get(product_info::OSP_VERSION),
        Frame::start(product_info::GET_ALL),
        get(0, 14),
        get(0, 15),
        Frame::start(settings::GET_ALL),
        get(1, 6),
        get(1, 7),
        get(1, 18),
        get(1, 23),
        Frame::start(status::GET_ALL),
        Frame::get(status::BATTERY_LEVEL),
        get(2, 9),
        get(2, 10),
        get(5, 5),
        get(5, 6),
        get(5, 7),
        get(5, 9),
        get(4, 22),
    ];
    for request in requests {
        if let Err(error) = print_exchange(case, &request).await {
            if matches!(error, case::Error::Disconnected) {
                return Err(error).context("dumping the protocol");
            }
            println!("!! {error}");
        }
    }
    Ok(())
}

async fn print_exchange(case: &Case, request: &Frame) -> Result<(), case::Error> {
    println!("-> {request}  # {} {:?}", request.function, request.operator);
    case.exchange(request, |frame| {
        println!("<- {frame}  # {} {:?}", frame.function, frame.operator);
    })
    .await?;
    Ok(())
}

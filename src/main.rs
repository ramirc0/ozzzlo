//! Control Ozlo Sleepbuds 2 from a Linux desktop, as a CLI and a TUI.

mod case;
mod logging;
mod osp;
mod snapshot;
mod tui;

use std::time::Duration;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};

use case::Case;
use osp::audio::PlayState;
use osp::settings::{Alarm, AutoStart, BluetoothMode, CaseClock, Days, Sound};
use osp::{Frame, Function, Operator, audio, product_info, settings, status};
use snapshot::Snapshot;

const MODE_CHANGE_POLLS: u32 = 8;

/// Control Ozlo Sleepbuds from a Linux desktop.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Echo the log to stderr. Repeat for more detail.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Print everything known about the case.
    Status,
    /// Print the charge of the case and each bud.
    Battery,
    /// List the sounds stored on the buds.
    Sounds,
    /// Play SOUND, or resume the current sound.
    Play {
        /// A sound's ID or the start of its name.
        sound: Option<String>,
    },
    /// Pause the current sound.
    Pause,
    /// Stop the current sound.
    Stop,
    /// Show the sound volume, or change it to LEVEL.
    Volume {
        /// The new level, from 0 to 100.
        #[arg(value_parser = clap::value_parser!(u8).range(0..=100))]
        level: Option<u8>,
    },
    /// Show the sleep timer, or set it to MINUTES or `off`.
    Timer {
        /// Minutes until playback stops, or `off`.
        duration: Option<String>,
    },
    /// Show or change when a sound starts once the buds leave the case.
    AutoStart {
        /// `immediately`, `off`, `asleep`, or a delay in minutes.
        mode: Option<AutoStart>,
    },
    /// Show or change whether the case accepts streamed audio.
    Bluetooth {
        /// `enabled`, `power-saving` or `disabled`.
        mode: Option<BluetoothMode>,
    },
    /// List the alarms, or change one.
    Alarm {
        #[command(subcommand)]
        action: Option<AlarmAction>,
    },
    /// Query every readable function and print the raw replies.
    Dump,
    /// Send one Get frame given as spaced hex, such as `02 02 01 00`.
    Raw {
        /// The frame, without the BLE prefix byte.
        hex: Vec<String>,
    },
}

#[derive(Subcommand)]
enum AlarmAction {
    /// Create an alarm at TIME.
    Add {
        /// Time of day as HH:MM, on a 24-hour clock.
        time: String,
        /// `weekdays`, `weekends`, `daily`, `once`, or names such as `mon,wed`.
        #[arg(long, default_value = "daily")]
        days: Days,
        /// An alarm sound's ID or the start of its name.
        #[arg(long)]
        sound: Option<String>,
        /// Alarm volume, from 0 to 100.
        #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u8).range(0..=100))]
        volume: u8,
    },
    /// Change the alarm in slot INDEX.
    Edit {
        index: u8,
        /// Time of day as HH:MM, on a 24-hour clock.
        #[arg(long)]
        time: Option<String>,
        /// `weekdays`, `weekends`, `daily`, `once`, or names such as `mon,wed`.
        #[arg(long)]
        days: Option<Days>,
        /// An alarm sound's ID or the start of its name.
        #[arg(long)]
        sound: Option<String>,
        /// Alarm volume, from 0 to 100.
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=100))]
        volume: Option<u8>,
    },
    /// Delete the alarm in slot INDEX.
    Delete { index: u8 },
    /// Turn on the alarm in slot INDEX.
    On { index: u8 },
    /// Turn off the alarm in slot INDEX.
    Off { index: u8 },
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let cli = Cli::parse();
    // The interface owns the terminal, so it never echoes the log.
    let echo = if cli.command.is_some() {
        cli.verbose
    } else {
        0
    };
    if let Err(error) = logging::start(echo) {
        eprintln!("warning: logging is off: {error:#}");
    }
    log::info!("ozzzlo {} started", env!("CARGO_PKG_VERSION"));
    let outcome = match cli.command {
        Some(command) => run(command).await,
        None => tui::run().await,
    };
    if let Err(error) = outcome {
        log::error!("{error:?}");
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

async fn run(command: Command) -> anyhow::Result<()> {
    let case = Case::connect().await.context("connecting to the case")?;
    let mut snapshot = Snapshot::default();
    match command {
        Command::Status => {
            survey(&case, &mut snapshot).await?;
            println!("{snapshot}");
        }
        Command::Battery => {
            ask(&case, &mut snapshot, &Frame::get(status::BATTERY_LEVEL)).await?;
            let level = snapshot
                .battery
                .context("the battery reply was malformed")?;
            println!(
                "case {}%  left {}%  right {}%",
                level.case, level.left, level.right
            );
        }
        Command::Sounds => {
            ask(&case, &mut snapshot, &Frame::get(settings::SOUND_LOOP_LIST)).await?;
            ask(&case, &mut snapshot, &Frame::get(audio::PLAYING_LOOP)).await?;
            let current = snapshot.playback.map(|playback| playback.loop_id);
            for sound in &snapshot.sound_loops {
                let marker = if current == Some(sound.id) { "*" } else { " " };
                println!("{marker} {:3}  {}", sound.id, snapshot.loop_name(sound.id));
            }
        }
        Command::Play { sound } => play(&case, &mut snapshot, sound).await?,
        Command::Pause | Command::Stop => {
            ask(&case, &mut snapshot, &Frame::get(audio::PLAYING_LOOP)).await?;
            let current = snapshot
                .playback
                .context("the playback reply was malformed")?;
            let request = match command {
                Command::Pause => audio::pause(current.loop_id),
                _ => audio::stop(current.loop_id),
            };
            ask(&case, &mut snapshot, &request).await?;
            print_playback(&snapshot);
        }
        Command::Volume { level } => {
            let request = match level {
                Some(level) => audio::change_volume_to(level),
                None => Frame::get(audio::SOUND_LOOP_VOLUME),
            };
            ask(&case, &mut snapshot, &request).await?;
            let volume = snapshot.volume.context("the volume reply was malformed")?;
            println!("volume {volume}");
        }
        Command::Timer { duration } => {
            let request = match duration.as_deref() {
                None => Frame::get(settings::SLEEP_TIMER),
                Some("off") => settings::sleep_after(0),
                Some(minutes) => settings::sleep_after(
                    minutes
                        .parse()
                        .with_context(|| format!("`{minutes}` is not minutes or `off`"))?,
                ),
            };
            ask(&case, &mut snapshot, &request).await?;
            match snapshot.sleep_timer_minutes {
                Some(0) => println!("sleep timer off"),
                Some(minutes) => println!("sleep timer {minutes} min"),
                None => bail!("the sleep timer reply was malformed"),
            }
        }
        Command::AutoStart { mode } => {
            let request = match mode {
                Some(mode) => settings::auto_start(mode),
                None => Frame::get(settings::AUTO_START),
            };
            ask(&case, &mut snapshot, &request).await?;
            let mode = snapshot
                .auto_start
                .context("the auto-start reply was malformed")?;
            println!("auto-start {mode}");
        }
        Command::Bluetooth { mode } => {
            ask(&case, &mut snapshot, &Frame::get(settings::BLUETOOTH_MODE)).await?;
            if let Some(wanted) = mode {
                ask(&case, &mut snapshot, &settings::bluetooth(wanted)).await?;
                // The case switches modes a few seconds after it acknowledges.
                for _ in 0..MODE_CHANGE_POLLS {
                    if snapshot.bluetooth_mode == Some(wanted) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    ask(&case, &mut snapshot, &Frame::get(settings::BLUETOOTH_MODE)).await?;
                }
            }
            let mode = snapshot
                .bluetooth_mode
                .context("the Bluetooth mode reply was malformed")?;
            println!("bluetooth {mode}");
        }
        Command::Alarm { action } => {
            ask(&case, &mut snapshot, &Frame::get(settings::ALARMS)).await?;
            ask(
                &case,
                &mut snapshot,
                &Frame::get(settings::ALARM_SOUND_LIST),
            )
            .await?;
            if let Some(action) = action {
                change_alarm(&case, &mut snapshot, action).await?;
            }
            print_alarms(&snapshot);
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

/// Sends `request` and folds every frame that arrives into `snapshot`.
async fn ask(case: &Case, snapshot: &mut Snapshot, request: &Frame) -> anyhow::Result<()> {
    case.exchange(request, |frame| snapshot.apply(frame))
        .await
        .with_context(|| format!("asking for {}", request.function))?;
    Ok(())
}

/// Asks the case for its full state.
async fn survey(case: &Case, snapshot: &mut Snapshot) -> anyhow::Result<()> {
    for request in Snapshot::survey() {
        ask(case, snapshot, &request).await?;
    }
    Ok(())
}

async fn play(case: &Case, snapshot: &mut Snapshot, sound: Option<String>) -> anyhow::Result<()> {
    ask(case, snapshot, &Frame::get(settings::SOUND_LOOP_LIST)).await?;
    ask(case, snapshot, &Frame::get(audio::PLAYING_LOOP)).await?;
    let target = match sound {
        Some(wanted) => sound_matching(&wanted, &snapshot.sound_loops, settings::loop_title)?,
        None => {
            snapshot
                .playback
                .context("the playback reply was malformed")?
                .loop_id
        }
    };
    for request in snapshot.requests_to_play(target) {
        ask(case, snapshot, &request).await?;
    }
    print_playback(snapshot);
    Ok(())
}

fn print_playback(snapshot: &Snapshot) {
    if let Some(playback) = snapshot.playback {
        let state = match playback.state {
            PlayState::Playing => "playing",
            PlayState::Paused => "paused",
            PlayState::Stopped => "stopped",
        };
        println!("{} {state}", snapshot.loop_name(playback.loop_id));
    }
}

/// Finds the sound whose ID is `wanted` or whose name starts with it.
fn sound_matching(
    wanted: &str,
    sounds: &[Sound],
    title: fn(u8) -> Option<&'static str>,
) -> anyhow::Result<u8> {
    let wanted = wanted.to_lowercase();
    let found = sounds.iter().find(|sound| {
        wanted.parse() == Ok(sound.id)
            || sound.label.to_lowercase() == wanted
            || title(sound.id).is_some_and(|title| title.to_lowercase().starts_with(&wanted))
    });
    match found {
        Some(sound) => Ok(sound.id),
        None => bail!("no sound on the buds matches `{wanted}`"),
    }
}

fn time_of_day(text: &str) -> anyhow::Result<(u8, u8)> {
    let parsed = text
        .split_once(':')
        .and_then(|(hour, minute)| Some((hour.parse().ok()?, minute.parse().ok()?)));
    match parsed {
        Some((hour, minute)) if hour < 24 && minute < 60 => Ok((hour, minute)),
        _ => bail!("`{text}` is not a time such as 07:30"),
    }
}

async fn change_alarm(
    case: &Case,
    snapshot: &mut Snapshot,
    action: AlarmAction,
) -> anyhow::Result<()> {
    let existing = |index: u8| {
        let alarm = snapshot.alarms.iter().find(|alarm| alarm.index == index);
        alarm
            .copied()
            .with_context(|| format!("no alarm in slot {index}"))
    };
    let alarm_sound =
        |wanted: &str| sound_matching(wanted, &snapshot.alarm_sounds, settings::alarm_sound_title);
    let alarm = match action {
        AlarmAction::Delete { index } => {
            existing(index)?;
            ask(case, snapshot, &settings::delete_alarm(index)).await?;
            return ask(case, snapshot, &Frame::get(settings::ALARMS)).await;
        }
        AlarmAction::On { index } => Alarm {
            enabled: true,
            ..existing(index)?
        },
        AlarmAction::Off { index } => Alarm {
            enabled: false,
            ..existing(index)?
        },
        AlarmAction::Add {
            time,
            days,
            sound,
            volume,
        } => {
            let (hour, minute) = time_of_day(&time)?;
            let index = snapshot
                .free_alarm_slot()
                .context("every alarm slot is taken")?;
            let sound = match sound {
                Some(wanted) => alarm_sound(&wanted)?,
                None => {
                    snapshot
                        .alarm_sounds
                        .first()
                        .context("the buds hold no alarm sound")?
                        .id
                }
            };
            Alarm {
                index,
                hour,
                minute,
                enabled: true,
                volume,
                days,
                sound,
            }
        }
        AlarmAction::Edit {
            index,
            time,
            days,
            sound,
            volume,
        } => {
            let mut alarm = existing(index)?;
            if let Some(time) = time {
                (alarm.hour, alarm.minute) = time_of_day(&time)?;
            }
            if let Some(sound) = sound {
                alarm.sound = alarm_sound(&sound)?;
            }
            alarm.days = days.unwrap_or(alarm.days);
            alarm.volume = volume.unwrap_or(alarm.volume);
            alarm
        }
    };
    // Alarms fire by the case's clock, which the vendor app sets every session.
    ask(case, snapshot, &settings::set_clock(&CaseClock::now())).await?;
    ask(case, snapshot, &settings::store_alarm(&alarm)).await
}

fn print_alarms(snapshot: &Snapshot) {
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

async fn dump(case: &Case) -> anyhow::Result<()> {
    let get = |block, id| Frame::get(Function { block, id });
    let requests = [
        Frame::get(product_info::OSP_VERSION),
        Frame::start(product_info::GET_ALL),
        get(0, 14),
        get(0, 15),
        Frame::start(settings::GET_ALL),
        Frame::start(status::GET_ALL),
        get(2, 10),
        Frame::get(audio::NOW_PLAYING),
        Frame::get(audio::PLAYING_LOOP),
        Frame::get(audio::SOUND_LOOP_VOLUME),
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
    println!(
        "-> {request}  # {} {:?}",
        request.function, request.operator
    );
    case.exchange(request, |frame| {
        println!("<- {frame}  # {} {:?}", frame.function, frame.operator);
    })
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_definition_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn sounds_match_by_id_label_or_name_prefix() {
        let sounds = [
            Sound {
                id: 6,
                label: "M6".to_string(),
            },
            Sound {
                id: 65,
                label: "M65".to_string(),
            },
        ];
        let find = |wanted| sound_matching(wanted, &sounds, settings::loop_title).ok();
        assert_eq!(find("6"), Some(6));
        assert_eq!(find("rain"), Some(6));
        assert_eq!(find("m65"), Some(65));
        assert_eq!(find("thunder"), None);
    }

    #[test]
    fn times_of_day_need_valid_hours_and_minutes() {
        assert_eq!(time_of_day("07:30").ok(), Some((7, 30)));
        assert!(time_of_day("24:00").is_err());
        assert!(time_of_day("7").is_err());
    }
}

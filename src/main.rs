mod case;
mod osp;

use clap::{Parser, Subcommand};

use case::Case;
use osp::{Frame, status};

/// Control Ozlo Sleepbuds from a Linux desktop.
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the charge of the case and each bud.
    Battery,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> case::Result<()> {
    let cli = Cli::parse();
    let case = Case::connect().await?;
    match cli.command {
        Command::Battery => {
            let reply = case.request(&Frame::get(status::BATTERY_LEVEL)).await?;
            let [case, left, right, ..] = reply.payload[..] else {
                return Err("battery reply is too short".into());
            };
            println!("case {case}%  left {left}%  right {right}%");
        }
    }
    Ok(())
}

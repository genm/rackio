mod remote;
mod runtime;

use chrono::Utc;
use clap::{Parser, Subcommand};
use remote::RemoteHistoryResolution;
use runtime::{LocalCommand, app_paths, request_local, run_daemon};

#[derive(Debug, Parser)]
#[command(name = "rackio", version, about = "P2P system monitor agent")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Daemon,
    Status,
    Fleet,
    History {
        endpoint_id: String,
        #[arg(long, default_value_t = 24, value_parser = clap::value_parser!(u16).range(1..=168))]
        hours: u16,
    },
    Pairing {
        #[command(subcommand)]
        command: PairingCommand,
    },
    Peer {
        #[command(subcommand)]
        command: PeerCommand,
    },
    Relay {
        #[command(subcommand)]
        command: RelayCommand,
    },
    Doctor,
}

#[derive(Debug, Subcommand)]
enum PairingCommand {
    Create,
    Import { bundle: String },
}

#[derive(Debug, Subcommand)]
enum PeerCommand {
    List,
    Revoke { endpoint_id: String },
}

#[derive(Debug, Subcommand)]
enum RelayCommand {
    /// Set a self-hosted relay URL, or pass `direct-only` to remove it.
    Set { url: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let paths = app_paths()?;
    match Cli::parse().command {
        Command::Daemon => run_daemon(paths).await?,
        Command::Status => print_response(request_local(&paths, LocalCommand::Status).await?)?,
        Command::Fleet => {
            print_response(request_local(&paths, LocalCommand::FleetSnapshot).await?)?;
        }
        Command::History { endpoint_id, hours } => {
            let to_ms = Utc::now().timestamp_millis();
            let from_ms = to_ms.saturating_sub(i64::from(hours) * 60 * 60 * 1_000);
            print_response(
                request_local(
                    &paths,
                    LocalCommand::QueryHistory {
                        endpoint_id,
                        from_ms,
                        to_ms,
                        resolution: RemoteHistoryResolution::Minute,
                    },
                )
                .await?,
            )?;
        }
        Command::Pairing {
            command: PairingCommand::Create,
        } => print_response(request_local(&paths, LocalCommand::PairingCreate).await?)?,
        Command::Pairing {
            command: PairingCommand::Import { bundle },
        } => {
            print_response(request_local(&paths, LocalCommand::PairingImport { bundle }).await?)?;
        }
        Command::Peer {
            command: PeerCommand::List,
        } => print_response(request_local(&paths, LocalCommand::PeerList).await?)?,
        Command::Peer {
            command: PeerCommand::Revoke { endpoint_id },
        } => {
            print_response(request_local(&paths, LocalCommand::PeerRevoke { endpoint_id }).await?)?;
        }
        Command::Relay {
            command: RelayCommand::Set { url },
        } => {
            let relay_url = (url != "direct-only").then_some(url);
            print_response(request_local(&paths, LocalCommand::RelaySet { relay_url }).await?)?;
        }
        Command::Doctor => print_response(request_local(&paths, LocalCommand::Doctor).await?)?,
    }
    Ok(())
}

fn print_response(response: runtime::LocalResponse) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(&response)?);
    if response.ok {
        Ok(())
    } else {
        anyhow::bail!(
            "{}",
            response
                .error
                .unwrap_or_else(|| String::from("daemon request failed"))
        )
    }
}

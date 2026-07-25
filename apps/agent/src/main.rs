mod runtime;

use clap::{Parser, Subcommand};
use runtime::{LocalCommand, app_paths, request_local, run_daemon};

#[derive(Debug, Parser)]
#[command(name = "tray-monitor", version, about = "P2P system monitor agent")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Daemon,
    Status,
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
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
}

#[derive(Debug, Subcommand)]
enum PairingCommand {
    Create,
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

#[derive(Debug, Subcommand)]
enum ServiceCommand {
    Install,
    Start,
    Stop,
    Uninstall,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let paths = app_paths()?;
    match Cli::parse().command {
        Command::Daemon => run_daemon(paths).await?,
        Command::Status => print_response(request_local(&paths, LocalCommand::Status).await?)?,
        Command::Pairing {
            command: PairingCommand::Create,
        } => print_response(request_local(&paths, LocalCommand::PairingCreate).await?)?,
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
        Command::Service { command } => {
            let action = match command {
                ServiceCommand::Install => "install",
                ServiceCommand::Start => "start",
                ServiceCommand::Stop => "stop",
                ServiceCommand::Uninstall => "uninstall",
            };
            anyhow::bail!(
                "service {action} is package-manager owned; use packaging/README.md for this OS"
            );
        }
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

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
    /// Inspect and change this machine's local health thresholds.
    Alerts {
        #[command(subcommand)]
        command: AlertCommand,
    },
    /// Control the fixed UDP port this machine listens on.
    ListenPort {
        #[command(subcommand)]
        command: ListenPortCommand,
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
enum AlertCommand {
    /// Show every effective rule, including the ones switched off.
    List,
    /// Change one rule. Omitted options keep their current value, and a rule
    /// Rackio does not ship needs `--metric`, `--comparison`, `--threshold`
    /// and `--severity` the first time it is defined.
    Set {
        id: String,
        #[arg(long, value_parser = finite_threshold)]
        threshold: Option<f64>,
        /// How many two-second samples in a row the condition must hold.
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
        samples: Option<u32>,
        #[arg(long, value_parser = ["warning", "critical"])]
        severity: Option<String>,
        #[arg(long, value_parser = rackio_core::ALERT_METRICS)]
        metric: Option<String>,
        #[arg(long, value_parser = ["at-or-above", "at-or-below"])]
        comparison: Option<String>,
    },
    /// Switch one rule off without losing its level.
    Disable { id: String },
    /// Switch one rule back on.
    Enable { id: String },
    /// Drop changes to one rule, or to every rule, restoring shipped levels.
    Reset {
        /// Omit to reset every rule on this machine.
        id: Option<String>,
    },
    /// Stop evaluating local thresholds on this machine entirely.
    Off,
    /// Evaluate local thresholds again.
    On,
}

#[derive(Debug, Subcommand)]
enum ListenPortCommand {
    /// Set a fixed listen port, or pass `ephemeral` to let the OS choose one.
    ///
    /// A fixed port keeps this machine's direct addresses stable across
    /// restarts, so already paired viewers reconnect without re-pairing.
    Set { port: String },
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
        Command::ListenPort {
            command: ListenPortCommand::Set { port },
        } => {
            let bind_port = if port == "ephemeral" {
                None
            } else {
                Some(port.parse::<u16>().map_err(|_| {
                    anyhow::anyhow!("listen port must be a number from 1 to 65535, or `ephemeral`")
                })?)
            };
            print_response(request_local(&paths, LocalCommand::BindPortSet { bind_port }).await?)?;
        }
        Command::Alerts { command } => {
            let alerts = match command {
                AlertCommand::List => runtime::AlertCommand::List,
                AlertCommand::Set {
                    id,
                    threshold,
                    samples,
                    severity,
                    metric,
                    comparison,
                } => runtime::AlertCommand::Set {
                    id,
                    metric,
                    comparison: comparison.as_deref().map(comparison_from),
                    threshold,
                    consecutive_samples: samples,
                    severity: severity.as_deref().map(severity_from),
                },
                AlertCommand::Disable { id } => {
                    runtime::AlertCommand::RuleEnabled { id, enabled: false }
                }
                AlertCommand::Enable { id } => {
                    runtime::AlertCommand::RuleEnabled { id, enabled: true }
                }
                AlertCommand::Reset { id } => runtime::AlertCommand::Reset { id },
                AlertCommand::Off => runtime::AlertCommand::Enabled { enabled: false },
                AlertCommand::On => runtime::AlertCommand::Enabled { enabled: true },
            };
            print_response(request_local(&paths, LocalCommand::Alerts { alert: alerts }).await?)?;
        }
        Command::Doctor => print_response(request_local(&paths, LocalCommand::Doctor).await?)?,
    }
    Ok(())
}

/// Reject `nan` and `inf` here, at the boundary that can still explain itself.
///
/// JSON has no way to carry them, so a non-finite threshold would reach the
/// daemon as an absent field and be reported as a successful change that
/// changed nothing.
fn finite_threshold(value: &str) -> Result<f64, String> {
    let parsed: f64 = value
        .parse()
        .map_err(|_| format!("`{value}` is not a number"))?;
    if parsed.is_finite() {
        Ok(parsed)
    } else {
        Err(format!("`{value}` is not a finite threshold"))
    }
}

/// The CLI spells the comparison the way an operator reads a threshold; clap
/// has already rejected anything else.
fn comparison_from(value: &str) -> rackio_core::Comparison {
    if value == "at-or-below" {
        rackio_core::Comparison::LessThanOrEqual
    } else {
        rackio_core::Comparison::GreaterThanOrEqual
    }
}

fn severity_from(value: &str) -> rackio_core::NodeState {
    if value == "critical" {
        rackio_core::NodeState::Critical
    } else {
        rackio_core::NodeState::Warning
    }
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

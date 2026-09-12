#![deny(unsafe_code)]
#![deny(
  clippy::unwrap_used,
  clippy::expect_used,
  clippy::panic,
  clippy::unreachable
)]
#![deny(clippy::arithmetic_side_effects)]
#![deny(clippy::todo)]
#![deny(clippy::allow_attributes_without_reason)]

pub mod agent;
pub mod config;
pub mod endpoint;
pub mod host;
pub mod log;
pub mod provider;
pub mod runtime;
pub mod secret;
pub mod tooling;
pub mod watch;

pub async fn run() -> anyhow::Result<()> {
  log::init();

  let cli = config::Cli::load()?;
  tracing::info!(command = ?cli.command, "omw starting");

  let result = match cli.command {
    config::Command::Run { args } => {
      let config = args.load_config()?;
      agent::run_agents(&config, args.watch()).await
    }
    config::Command::Loop { args } => {
      let config = args.load_config()?;
      agent::loop_agents(&config, args.watch()).await
    }
    config::Command::Schema { output } => config::generate_schema(&output),
  };

  if let Err(error) = &result {
    tracing::error!(error = %error, "omw terminated with an error");
  }

  result
}

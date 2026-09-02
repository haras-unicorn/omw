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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  omw::log::init();

  let cli = omw::config::Cli::load()?;
  tracing::info!(
    command = ?cli.command,
    config = ?cli.resolve_config_path(),
    "omw starting"
  );

  let result = match cli.command {
    omw::config::Command::Run => {
      let config = cli.load_config()?;
      omw::agent::run_agents(&config).await
    }
    omw::config::Command::Loop => {
      let config = cli.load_config()?;
      omw::agent::loop_agents(&config).await
    }
    omw::config::Command::Schema { output } => {
      omw::config::generate_schema(&output)
    }
  };

  if let Err(error) = &result {
    tracing::error!(error = %error, "omw terminated with an error");
  }
  result
}

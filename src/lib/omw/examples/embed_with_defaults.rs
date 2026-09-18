//! Embed `omw` with defaults: load a file `Config`, install TLS once,
//! then run every agent once with the built-in registries.
//!
//! Runnable version of the `docs/library.md` embed snippet. The inline
//! TOML declares zero agents, so this exits 0 with no keys or network.

use std::sync::OnceLock;

use omw::prelude::*;

fn install_tls() {
  static INSTALLED: OnceLock<()> = OnceLock::new();
  INSTALLED.get_or_init(|| {
    if rustls::crypto::CryptoProvider::get_default().is_none()
      && rustls::crypto::ring::default_provider()
        .install_default()
        .is_err()
    {
      tracing::warn!(
        "rustls crypto provider already installed by another crate"
      );
    }
  });
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  install_tls();
  let dir = tempfile::tempdir()?;
  let path = dir.path().join("omw.toml");
  std::fs::write(&path, "")?;
  let raw: String = std::fs::read_to_string(&path)?;
  let cfg: Config = toml::from_str(&raw)?;
  let registries = Registries::default();
  run_agents(&cfg, false, &registries).await
}

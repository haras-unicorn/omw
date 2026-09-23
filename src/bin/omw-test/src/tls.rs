use std::sync::OnceLock;

/// Install `ring` as the process-wide `rustls` default crypto backend, once.
/// `reqwest` with `rustls-no-provider` panics on client construction without
/// an installed backend, so this runs before every client is created. Only
/// errors when another backend already owns the default, which cannot happen
/// from our own code paths.
pub fn init() {
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

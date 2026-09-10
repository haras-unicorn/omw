//! A secret string wrapper that redacts on `Debug` and `Serialize`.
//!
//! The bytes live in a fixed-size `Box<[u8]>` (no realloc moves), are
//! `mlock`ed against swap (fail-closed at construction), excluded from
//! core dumps on a best-effort basis, and zeroized before `munlock` on
//! drop. Deserializes transparently from a plain string (TOML/env), but
//! never renders the inner value through `Debug` or `Serialize`, so
//! `tracing` fields using `?` and config debug output cannot leak it.
//! Call [`Secret::expose`] only at the point of use.

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

#[derive(PartialEq, Eq)]
pub struct Secret(Box<[u8]>);

impl Secret {
  pub fn new(value: String) -> anyhow::Result<Self> {
    let mut bytes: Box<[u8]> = value.into_bytes().into_boxed_slice();
    lock(&bytes).map_err(|error| {
      anyhow::anyhow!("mlock failed for secret (check RLIMIT_MEMLOCK): {error}")
    })?;
    dontdump(&mut bytes);
    Ok(Self(bytes))
  }

  pub fn expose(&self) -> &str {
    // The bytes came from a `String` and are only mutated by `zeroize`
    // in `Drop`, at which point no borrows can exist.
    std::str::from_utf8(&self.0).unwrap_or_default()
  }
}

impl Clone for Secret {
  fn clone(&self) -> Self {
    let bytes = self.0.clone();
    if let Err(error) = lock(&bytes) {
      tracing::warn!(
        error = %error,
        "mlock failed for cloned secret, continuing unlocked"
      );
    }
    Self(bytes)
  }
}

impl Drop for Secret {
  fn drop(&mut self) {
    self.0.zeroize();
    unlock(&self.0);
  }
}

impl<'de> Deserialize<'de> for Secret {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: serde::Deserializer<'de>,
  {
    let value = String::deserialize(deserializer)?;
    Secret::new(value).map_err(serde::de::Error::custom)
  }
}

#[allow(
  unsafe_code,
  reason = "mlock needs a raw pointer; the slice is a live Secret buffer for the call"
)]
fn lock(bytes: &[u8]) -> std::io::Result<()> {
  if bytes.is_empty() {
    return Ok(());
  }
  unsafe { os_memlock::mlock(bytes.as_ptr().cast(), bytes.len()) }
}

#[allow(
  unsafe_code,
  reason = "munlock needs a raw pointer; the slice is a live Secret buffer for the call"
)]
fn unlock(bytes: &[u8]) {
  if bytes.is_empty() {
    return;
  }
  let _ = unsafe { os_memlock::munlock(bytes.as_ptr().cast(), bytes.len()) };
}

#[allow(
  unsafe_code,
  reason = "madvise needs a raw pointer; the slice is a live Secret buffer for the call"
)]
fn dontdump(bytes: &mut [u8]) {
  if bytes.is_empty() {
    return;
  }
  let result = unsafe {
    os_memlock::madvise_dontdump(bytes.as_mut_ptr().cast(), bytes.len())
  };
  if let Err(error) = result
    && error.kind() != std::io::ErrorKind::Unsupported
  {
    tracing::warn!(error = %error, "madvise dontdump failed for secret");
  }
}

impl schemars::JsonSchema for Secret {
  fn schema_name() -> std::borrow::Cow<'static, str> {
    std::borrow::Cow::Borrowed("Secret")
  }

  fn json_schema(
    _generator: &mut schemars::SchemaGenerator,
  ) -> schemars::Schema {
    schemars::json_schema!({
      "type": "string",
    })
  }
}

impl std::fmt::Debug for Secret {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "<redacted>")
  }
}

impl Serialize for Secret {
  fn serialize<S: serde::Serializer>(
    &self,
    serializer: S,
  ) -> Result<S::Ok, S::Error> {
    serializer.serialize_str("<redacted>")
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn debug_and_serialize_redact() -> anyhow::Result<()> {
    let secret = Secret::new("sk-test".to_string())?;
    assert_eq!(format!("{secret:?}"), "<redacted>");
    assert_eq!(
      serde_json::to_value(&secret)?,
      serde_json::Value::String("<redacted>".to_string())
    );
    assert_eq!(secret.expose(), "sk-test");
    Ok(())
  }

  #[test]
  fn deserializes_from_plain_string() -> anyhow::Result<()> {
    let secret: Secret = serde_json::from_value(serde_json::json!("sk-test"))?;
    assert_eq!(secret.expose(), "sk-test");
    Ok(())
  }

  #[test]
  fn deserializes_option_and_map_shapes() -> anyhow::Result<()> {
    #[derive(Deserialize)]
    struct Outer {
      #[serde(default)]
      token: Option<Secret>,
      #[serde(default)]
      env: std::collections::HashMap<String, Secret>,
    }
    let outer: Outer = serde_json::from_value(serde_json::json!({
      "token": "sk-test",
      "env": { "FOO": "bar" },
    }))?;
    assert_eq!(outer.token.as_ref().map(|s| s.expose()), Some("sk-test"));
    assert_eq!(outer.env["FOO"].expose(), "bar");
    let outer: Outer = serde_json::from_value(serde_json::json!({}))?;
    assert!(outer.token.is_none());
    assert!(outer.env.is_empty());
    Ok(())
  }

  #[test]
  fn empty_secret_skips_lock() -> anyhow::Result<()> {
    let secret = Secret::new(String::new())?;
    assert_eq!(secret.expose(), "");
    Ok(())
  }

  #[test]
  fn clone_preserves_value_and_redacts() -> anyhow::Result<()> {
    let secret = Secret::new("sk-test".to_string())?;
    let cloned = secret.clone();
    assert_eq!(secret, cloned);
    assert_eq!(cloned.expose(), "sk-test");
    assert_eq!(format!("{cloned:?}"), "<redacted>");
    Ok(())
  }
}

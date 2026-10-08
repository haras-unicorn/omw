//! Case-aware `OMW__` environment overlay.
//!
//! `config`'s own environment source lowercases *every* path segment so that
//! uppercase environment keys match the lowercase config model. That breaks
//! free-form map keys the user chose: an MCP server env var
//! (`OMW__TOOLING__MCP__ENV__GITHUB_TOKEN`) would become `github_token`, and a
//! provider request param (`OMW__PROVIDERS__OPENAI__PARAMS__max_tokens`)
//! would be folded too.
//!
//! This module stays deliberately dumb: it is handed a set of *opaque paths* —
//! full, config-root-relative patterns naming the free-form maps, with `*`
//! matching one dynamic segment (an entry's name) — and matches each
//! environment key against them. Segments at or before a matching pattern are
//! lowercased so they still line up with the config model; everything after it
//! keeps its case, because those are keys the user chose.
//!
//! Where the opaque paths come from is not this module's concern: the back
//! ends declare their own free-form params (see `opaque_fields` on the four
//! `Factory` traits) and the registries carry them, so nothing here knows an
//! impl's field names.

use serde_json::{Map, Value};

/// A free-form map path from the config root. Literal segments match
/// case-insensitively; `*` matches exactly one dynamic segment.
pub(crate) type OpaquePath = Vec<&'static str>;

/// Build the nested environment overlay for `prefix` (`__` separates
/// segments). Segments after one of `opaque` keep their case; the rest are
/// lowercased.
pub(crate) fn env_overlay(prefix: &str, opaque: &[OpaquePath]) -> Value {
  let vars: Vec<(String, String)> = std::env::vars_os()
    .filter_map(|(key, value)| {
      Some((key.into_string().ok()?, value.into_string().ok()?))
    })
    .collect();
  overlay(prefix, opaque, &vars)
}

/// The pure core of [`env_overlay`], taking the environment as a slice so it is
/// testable without touching the process.
fn overlay(
  prefix: &str,
  opaque: &[OpaquePath],
  vars: &[(String, String)],
) -> Value {
  let mut root = Map::new();
  for (key, value) in vars {
    let Some(segments) = split(prefix, key) else {
      continue;
    };
    if segments.is_empty() {
      continue;
    }
    let preserve_from = opaque
      .iter()
      .filter_map(|pattern| match_prefix(pattern, &segments))
      .max()
      .unwrap_or(segments.len());
    let path: Vec<String> = segments
      .iter()
      .enumerate()
      .map(|(index, segment)| {
        if index < preserve_from {
          segment.to_lowercase()
        } else {
          segment.clone()
        }
      })
      .collect();
    insert_nested(&mut root, &path, Value::String(value.clone()));
  }
  Value::Object(root)
}

/// Split `key` into its `prefix`-relative segments, or `None` when it does not
/// carry the prefix. The prefix itself is matched case-insensitively (the
/// config source lowercased it), everything after keeps its original case.
fn split(prefix: &str, key: &str) -> Option<Vec<String>> {
  let mut segments = key.split("__");
  let head = segments.next()?;
  if !head.eq_ignore_ascii_case(prefix) {
    return None;
  }
  Some(segments.map(str::to_owned).collect())
}

/// How many leading segments of `segments` `pattern` matches, or `None` when
/// it does not. A `*` segment matches any one segment; literals match
/// case-insensitively.
fn match_prefix(pattern: &[&str], segments: &[String]) -> Option<usize> {
  if pattern.len() > segments.len() {
    return None;
  }
  for (expected, actual) in pattern.iter().zip(segments) {
    if *expected != "*" && !expected.eq_ignore_ascii_case(actual.as_str()) {
      return None;
    }
  }
  Some(pattern.len())
}

/// Insert `value` at the dotted `path`, creating (or replacing a scalar with)
/// intermediate objects.
fn insert_nested(node: &mut Map<String, Value>, path: &[String], value: Value) {
  match path.split_first() {
    None => {}
    Some((last, [])) => {
      node.insert(last.clone(), value);
    }
    Some((head, rest)) => {
      let child = node
        .entry(head.clone())
        .or_insert_with(|| Value::Object(Map::new()));
      if !child.is_object() {
        *child = Value::Object(Map::new());
      }
      if let Some(map) = child.as_object_mut() {
        insert_nested(map, rest, value);
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  fn patterns() -> Vec<OpaquePath> {
    vec![
      vec!["providers", "*", "params"],
      vec!["tooling", "*", "env"],
      vec!["runtime", "*", "env"],
      vec!["memory", "*"],
    ]
  }

  fn run(vars: &[(&str, &str)]) -> Value {
    let vars: Vec<(String, String)> = vars
      .iter()
      .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
      .collect();
    overlay("OMW", &patterns(), &vars)
  }

  #[test]
  fn structural_keys_are_lowercased() {
    let value = run(&[("OMW__PROVIDERS__OPENAI__API_KEY", "from-env")]);
    assert_eq!(
      value,
      json!({ "providers": { "openai": { "api_key": "from-env" } } })
    );
  }

  #[test]
  fn mcp_env_keys_keep_their_case() {
    let value = run(&[("OMW__TOOLING__MCP__ENV__GITHUB_TOKEN", "tok")]);
    assert_eq!(
      value,
      json!({ "tooling": { "mcp": { "env": { "GITHUB_TOKEN": "tok" } } } })
    );
  }

  #[test]
  fn wasi_env_keys_keep_their_case() {
    let value = run(&[("OMW__RUNTIME__WASM__ENV__PATH", "/bin")]);
    assert_eq!(
      value,
      json!({ "runtime": { "wasm": { "env": { "PATH": "/bin" } } } })
    );
  }

  #[test]
  fn provider_params_keys_keep_their_case() {
    let value = run(&[("OMW__PROVIDERS__OPENAI__PARAMS__max_tokens", "10")]);
    assert_eq!(
      value,
      json!({ "providers": { "openai": { "params": { "max_tokens": "10" } } } })
    );
  }

  #[test]
  fn memory_keeps_the_key_but_not_the_agent() {
    let value = run(&[("OMW__MEMORY__ALICE__FavoriteColor", "blue")]);
    assert_eq!(
      value,
      json!({ "memory": { "alice": { "FavoriteColor": "blue" } } })
    );
  }

  #[test]
  fn unknown_fields_stay_lowercased() {
    let value = run(&[("OMW__TOOLING__MCP__COMMAND", "npx")]);
    assert_eq!(value, json!({ "tooling": { "mcp": { "command": "npx" } } }));
  }

  #[test]
  fn nested_keys_after_a_pattern_keep_their_case() {
    let value = run(&[("OMW__TOOLING__MCP__ENV__A__B", "x")]);
    assert_eq!(
      value,
      json!({ "tooling": { "mcp": { "env": { "A": { "B": "x" } } } } })
    );
  }

  #[test]
  fn prefix_is_matched_case_insensitively() {
    let value = run(&[("omw__TUNABLES__inbox_bound", "16")]);
    assert_eq!(value, json!({ "tunables": { "inbox_bound": "16" } }));
  }

  #[test]
  fn non_matching_vars_are_ignored() {
    let value = run(&[("OTHER__FOO", "bar"), ("OMW", "x")]);
    assert_eq!(value, json!({}));
  }
}

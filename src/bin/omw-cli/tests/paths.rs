//! `omw` config-path, format-inference and `scaffold` output integration tests.

mod common;

use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn discovers_the_default_config_in_the_cwd() {
  let dir = tempdir().unwrap();
  std::fs::write(dir.path().join("omw.toml"), "").unwrap();
  common::omw()
    .current_dir(dir.path())
    .arg("run")
    .assert()
    .success();
}

#[test]
fn errors_when_no_default_config_exists() {
  let dir = tempdir().unwrap();
  common::omw()
    .current_dir(dir.path())
    .arg("run")
    .assert()
    .failure()
    .stderr(predicate::str::contains("no config found"));
}

#[test]
fn infers_yaml_and_json_formats() {
  let yaml = tempdir().unwrap();
  std::fs::write(yaml.path().join("omw.yaml"), "{}").unwrap();
  common::omw()
    .current_dir(yaml.path())
    .arg("run")
    .assert()
    .success();

  let json = tempdir().unwrap();
  std::fs::write(json.path().join("omw.json"), "{}").unwrap();
  common::omw()
    .current_dir(json.path())
    .arg("run")
    .assert()
    .success();
}

#[test]
fn explicit_format_parses_an_extensionless_file() {
  let dir = tempdir().unwrap();
  let path = dir.path().join("omw.conf");
  std::fs::write(&path, "{}").unwrap();

  common::omw()
    .args(["run", "--config"])
    .arg(&path)
    .args(["--format", "json"])
    .assert()
    .success();

  common::omw()
    .args(["run", "--config"])
    .arg(&path)
    .assert()
    .failure()
    .stderr(predicate::str::contains("cannot infer"));
}

#[test]
fn scaffold_writes_next_to_the_config_by_default() {
  let dir = tempdir().unwrap();
  let config = dir.path().join("omw.toml");
  std::fs::write(&config, "[providers.ghost]\nkind = \"nope\"\n").unwrap();
  common::omw()
    .args(["scaffold"])
    .arg(&config)
    .assert()
    .success();
  assert!(dir.path().join("omw.test.toml").is_file());
}

#[test]
fn scaffold_refuses_to_overwrite_without_force() {
  let dir = tempdir().unwrap();
  let config = dir.path().join("omw.toml");
  std::fs::write(&config, "[providers.ghost]\nkind = \"nope\"\n").unwrap();
  std::fs::write(dir.path().join("omw.test.toml"), "existing").unwrap();

  common::omw()
    .args(["scaffold"])
    .arg(&config)
    .assert()
    .failure()
    .stderr(predicate::str::contains("already exists"));

  common::omw()
    .args(["scaffold", "--force"])
    .arg(&config)
    .assert()
    .success();
}

#[test]
fn schema_creates_parent_directories() {
  let dir = tempdir().unwrap();
  let out = dir.path().join("nested/deeper/schema.json");
  common::omw()
    .args(["schema", "--output"])
    .arg(&out)
    .assert()
    .success();
  assert!(out.is_file());
}

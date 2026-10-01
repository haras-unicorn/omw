//! `omw-test` standard-stream (`-`, `/dev/stdout`) integration tests.

mod common;

#[test]
fn schema_streams_json_to_stdout() {
  for target in ["-", "/dev/stdout"] {
    let assert = common::omw_test()
      .args(["schema", "--output", target])
      .assert()
      .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
      stdout.trim_start().starts_with('{') && stdout.trim_end().ends_with('}'),
      "expected a JSON document on stdout, got: {stdout}"
    );
  }
}

//! User-supplied path conventions: treat `-`, `/dev/stdin`/`/dev/stdout`,
//! `/dev/fd/N` and `/proc/self/fd/N` as the standard streams instead of files.
//!
//! This is a CLI concern (see `AGENTS.md`): the library never sees argv paths,
//! so the standard-stream convention lives with the binaries.

use std::io::Write as _;
use std::path::Path;

use anyhow::{Context as _, Result};

/// Whether `path` names standard input: `-`, or a stdin device path.
pub fn is_stdin(path: &Path) -> bool {
  path == Path::new("-") || is_device(path, "stdin", "0")
}

/// Whether `path` names standard output: `-`, or a stdout device path.
pub fn is_stdout(path: &Path) -> bool {
  path == Path::new("-") || is_device(path, "stdout", "1")
}

/// Whether `path` is `/dev/<name>`, `/dev/fd/<fd>` or `/proc/self/fd/<fd>`.
fn is_device(path: &Path, name: &str, fd: &str) -> bool {
  let Some(text) = path.to_str() else {
    return false;
  };
  let dev = format!("/dev/{name}");
  let dev_fd = format!("/dev/fd/{fd}");
  let proc_fd = format!("/proc/self/fd/{fd}");
  [dev.as_str(), dev_fd.as_str(), proc_fd.as_str()].contains(&text)
}

/// Read `path` as UTF-8, from stdin when it names standard input.
pub fn read_to_string(path: &Path) -> Result<String> {
  if is_stdin(path) {
    std::io::read_to_string(std::io::stdin())
      .context("failed to read from stdin")
  } else {
    std::fs::read_to_string(path)
      .with_context(|| format!("failed to read {}", path.display()))
  }
}

/// Write `contents` to `path`, to stdout when it names standard output. For a
/// regular path the parent directory is created first.
pub fn write(path: &Path, contents: &str) -> Result<()> {
  if is_stdout(path) {
    let mut out = std::io::stdout().lock();
    out
      .write_all(contents.as_bytes())
      .context("failed to write to stdout")?;
    out.flush().context("failed to flush stdout")?;
    return Ok(());
  }
  if let Some(parent) = path.parent()
    && !parent.as_os_str().is_empty()
  {
    std::fs::create_dir_all(parent).with_context(|| {
      format!("failed to create directory {}", parent.display())
    })?;
  }
  std::fs::write(path, contents)
    .with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn recognizes_standard_input() {
    assert!(is_stdin(Path::new("-")));
    assert!(is_stdin(Path::new("/dev/stdin")));
    assert!(is_stdin(Path::new("/dev/fd/0")));
    assert!(is_stdin(Path::new("/proc/self/fd/0")));
    assert!(!is_stdin(Path::new("/dev/stdout")));
    assert!(!is_stdin(Path::new("omw.test.toml")));
  }

  #[test]
  fn recognizes_standard_output() {
    assert!(is_stdout(Path::new("-")));
    assert!(is_stdout(Path::new("/dev/stdout")));
    assert!(is_stdout(Path::new("/dev/fd/1")));
    assert!(is_stdout(Path::new("/proc/self/fd/1")));
    assert!(!is_stdout(Path::new("/dev/stdin")));
    assert!(!is_stdout(Path::new("omw.test.toml")));
  }

  #[test]
  fn read_to_string_reads_a_file() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("omw.test.toml");
    std::fs::write(&path, "x = 1")?;
    assert_eq!(read_to_string(&path)?, "x = 1");
    Ok(())
  }

  #[test]
  fn write_creates_parent_directories() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("nested/omw.test.toml");
    write(&path, "hello")?;
    assert_eq!(std::fs::read_to_string(&path)?, "hello");
    Ok(())
  }
}

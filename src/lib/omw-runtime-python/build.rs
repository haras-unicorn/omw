//! Build script: cross-compiles the bundled Python guest
//! (`src/wasm/omw-wasm-python-interpreter`) for `wasm32-wasip1` and embeds the
//! portable component via [`omw_build::build_component`]. The interpreter is the
//! largest guest, so it asks for `-Oz` to keep its vendored component under
//! crates.io's 10 MiB crate limit.

fn main() {
  omw_build::build_component(
    "omw-wasm-python-interpreter",
    omw_build::OptLevel::Size,
  );
}

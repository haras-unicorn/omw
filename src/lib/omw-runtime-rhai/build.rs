//! Build script: cross-compiles the bundled rhai guest
//! (`src/wasm/omw-wasm-rhai-interpreter`) for `wasm32-wasip1` and embeds the
//! portable component via [`omw_build::build_component`], optimized for speed.

fn main() {
  omw_build::build_component(
    "omw-wasm-rhai-interpreter",
    omw_build::OptLevel::Speed,
  );
}

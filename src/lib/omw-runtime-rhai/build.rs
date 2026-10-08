//! Build script: cross-compiles the bundled rhai guest
//! (`src/wasm/omw-wasm-rhai-interpreter`) for `wasm32-wasip2` and embeds the
//! portable component via [`omw_build::build_component`].

fn main() {
  omw_build::build_component("omw-wasm-rhai-interpreter");
}

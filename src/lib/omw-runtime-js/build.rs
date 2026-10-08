//! Build script: cross-compiles the bundled JavaScript guest
//! (`src/wasm/omw-wasm-js-interpreter`) for `wasm32-wasip2` and embeds the
//! portable component via [`omw_build::build_component`].

fn main() {
  omw_build::build_component("omw-wasm-js-interpreter");
}

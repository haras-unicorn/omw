//! Build script: cross-compiles the bundled test-only mock brain
//! (`src/wasm/omw-wasm-mock`) for `wasm32-wasip2` and embeds the portable
//! component via [`omw_build::build_component`].

fn main() {
  omw_build::build_component("omw-wasm-mock");
}

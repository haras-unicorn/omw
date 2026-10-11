//! Build script: cross-compiles the bundled test-only mock brain
//! (`src/wasm/omw-wasm-mock`) for `wasm32-wasip1` and embeds the portable
//! component via [`omw_build::build_component`], optimized for speed.

fn main() {
  omw_build::build_component("omw-wasm-mock", omw_build::OptLevel::Speed);
}

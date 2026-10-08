//! The bundled rhai interpreter component for the `omw` agent runtime.
//!
//! The component is cross-compiled from `src/wasm/omw-wasm-rhai-interpreter`
//! and embedded here as **portable wasm**, so the `omw` package carries no
//! interpreter bytes and the embedding binary stays portable across machines.
//! The engine compiles the component in-process at startup.

/// The portable rhai interpreter component.
pub const COMPONENT_WASM: &[u8] =
  include_bytes!(env!("OMW_WASM_RHAI_INTERPRETER_COMPONENT_WASM"));

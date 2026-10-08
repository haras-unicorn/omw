//! The bundled JavaScript interpreter component for the `omw` agent runtime.
//!
//! The component is cross-compiled from `src/wasm/omw-wasm-js-interpreter` and
//! embedded here as **portable wasm**, so the `omw` package carries no
//! interpreter bytes and the embedding binary stays portable across machines.
//! The engine compiles the component in-process at startup.

/// The portable JavaScript interpreter component.
pub const COMPONENT_WASM: &[u8] =
  include_bytes!(env!("OMW_WASM_JS_INTERPRETER_COMPONENT_WASM"));

//! The bundled test-only mock brain component for the `omw` agent runtime.
//!
//! The component is cross-compiled from `src/wasm/omw-wasm-mock` and embedded
//! here as **portable wasm**, so `omw`'s engine tests reuse it without any
//! host-specific `cwasm` baked in. The engine compiles it in-process.

/// The portable mock brain component.
pub const COMPONENT_WASM: &[u8] =
  include_bytes!(env!("OMW_WASM_MOCK_COMPONENT_WASM"));

/// The mock brain component printed as WAT, for the engine's WAT-loading tests.
pub const COMPONENT_WAT: &[u8] =
  include_bytes!(env!("OMW_WASM_MOCK_COMPONENT_WAT"));

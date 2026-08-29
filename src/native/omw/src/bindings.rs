//! Single bindgen! for the OMW WIT world(s).
//!
//! The `omw` world is the one every brain component (including the bundled
//! rhai interpreter) is built against. Host-side import traits and the guest
//! export accessors are all derived from this one macro.
//!
//! The provider/tooling resources are mapped via `with` to our registry
//! entries, so `ResourceTable` entries carry per-instance state.

wasmtime::component::bindgen!({
    world: "omw",
    path: "../../wit",
    with: {
        "omw:omw/provider.provider": crate::provider::ProviderEntry,
        "omw:omw/tooling.tooling": crate::tooling::ToolingEntry,
    },
});

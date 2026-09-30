//! Helpers shared by the per-category config schema unions.
//!
//! Each category (`provider`, `tooling`, `runtime`, `endpoint`) exposes a
//! `pub(crate)` schema type whose `JsonSchema` impl builds an `anyOf` of the
//! built-in kinds enabled in this build, plus the generic opaque escape hatch
//! (`ImplConfig`) so custom back ends still validate.

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};

/// One built-in impl's config: the back end's params plus a `kind` constant,
/// so an editor can discriminate the variants and complete the right fields.
pub(crate) fn kind_variant<T: JsonSchema>(
  generator: &mut SchemaGenerator,
  kind: &str,
) -> Schema {
  let params = generator.subschema_for::<T>();
  json_schema!({
    "allOf": [
      params,
      {
        "type": "object",
        "properties": { "kind": { "const": kind } },
        "required": ["kind"]
      }
    ]
  })
}

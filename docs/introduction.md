# OMW

{{#include ../README.md:body}}

## References

The machine-readable configuration schema, alongside the human-readable pages in
this book.

### Configuration schema

The deployment schema (`omw`), describing the openai provider, mcp tooling,
openai endpoint, and wasm runtime:

```json
{{#include ../assets/schema.json}}
```

The testing schema (`omw-test`), describing the mock provider/tooling/endpoint
and the wasm/rhai/js/python runtimes:

```json
{{#include ../assets/schema.test.json}}
```

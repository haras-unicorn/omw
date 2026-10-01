# 06-memory

A brain that starts from a seeded state. The shared config seeds an object under
`[memory.alice]` (`state = { model = "seed-42", retries = 3 }`); the brain reads
it back parsed (`memory_get_as` / `memoryGetAs` / `memory_get_as::<State>`) and
uses its `model` field as the model name, so the seed is visible in the trace.

```sh
omw-test run examples/06-memory
```

It asserts `memory_get("state")` followed by a `chat` with model `seed-42`. See
the [examples guide](../../docs/examples.md) for how the variants and assertions
work.

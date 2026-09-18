# 06-memory

A brain that starts from a seeded state. The shared config sets
`[memory.alice] handle = "seed-42"`; the brain reads `handle` before doing
anything and uses it as the model name, so the seed is visible in the trace.

```sh
omw-test run examples/06-memory
```

It asserts `memory_get("handle")` followed by a `chat` with model `seed-42`. See
the [examples guide](../../docs/examples.md) for how the variants and assertions
work.

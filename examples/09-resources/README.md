# 09-resources

A brain that subscribes to a tooling's resource list and to a single resource,
reads one, and reacts to scripted updates. The shared config drives the
[tooling mock](../../docs/testing/mocks/tooling.md): an initial list and
content, two ordered `resource_list_updates`, and one `resource_content_updates`
step, each gated on a trace event with `after`.

```sh
omw-test run examples/09-resources
```

The brain subscribes to the list, lists, and receives both gated list updates in
order, then subscribes to `mem://notes`, reads it, and receives the gated
content update (`v2`). It asserts the ordered `resource-list-updated` /
`resource-updated` events alongside the subscribe/read/unsubscribe calls. See
the [examples guide](../../docs/examples.md) for how the variants and assertions
work.

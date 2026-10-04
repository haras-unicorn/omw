# 12-race-cancels

Two subscriptions cancelled back to back.

The brain subscribes to a resource list and a single resource, then cancels both
in sequence. A cancellation emits its `closed{cancelled}` observation
**synchronously** inside the unsubscribe call, so the two closes are pinned to
the calls that caused them instead of racing each other from the pump tasks they
wake. Before this change the two `closed` events could interleave in a
scheduler-chosen order.

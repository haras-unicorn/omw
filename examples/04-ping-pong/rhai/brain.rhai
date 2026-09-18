// Two agents share this script. Without a `whoami` host call it cannot tell
// which name is its own, so it subscribes to both, pings both, and receives
// exactly one message: its self-addressed ping is always delivered because it
// subscribed to its own name first. The handles go to memory so a hot reload
// can reuse them, and are dropped explicitly to mirror the rust brain's RAII
// guards.
let sub_alice = omw::host::subscribe_agent("alice");
let sub_bob = omw::host::subscribe_agent("bob");
omw::host::memory_set("sub-alice", sub_alice);
omw::host::memory_set("sub-bob", sub_bob);
omw::host::send_agent("alice", "ping");
omw::host::send_agent("bob", "ping");
let event = omw::host::recv();
omw::host::send_agent("alice", "pong");
omw::host::send_agent("bob", "pong");
omw::host::unsubscribe_agent(sub_alice);
omw::host::unsubscribe_agent(sub_bob);
omw::host::log("info", event.kind);

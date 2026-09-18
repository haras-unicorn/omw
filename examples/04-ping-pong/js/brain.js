// Two agents share this script. Without a `whoami` host call it cannot tell
// which name is its own, so it subscribes to both, pings both, and receives
// exactly one message: its self-addressed ping is always delivered because it
// subscribed to its own name first. The handles go to memory so a hot reload
// can reuse them, and are dropped explicitly to mirror the rust brain's RAII
// guards.
const subAlice = omw.host.subscribeAgent("alice");
const subBob = omw.host.subscribeAgent("bob");
omw.host.memorySet("sub-alice", subAlice);
omw.host.memorySet("sub-bob", subBob);
omw.host.sendAgent("alice", "ping");
omw.host.sendAgent("bob", "ping");
const event = omw.host.recv();
omw.host.sendAgent("alice", "pong");
omw.host.sendAgent("bob", "pong");
omw.host.unsubscribeAgent(subAlice);
omw.host.unsubscribeAgent(subBob);
omw.host.log("info", event.kind);

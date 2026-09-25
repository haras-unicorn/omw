// Two agents share this script. `whoami` tells each agent its own name, so it
// subscribes only to itself and plays a deterministic ping-pong with its own
// inbox. The handle goes to memory so a hot reload can reuse it, and is dropped
// explicitly to mirror the rust brain's RAII guards.
const me = omw.host.whoami();
const sub = omw.host.subscribeAgent(me);
omw.host.memorySet("sub", sub);
omw.host.sendAgent(me, "ping");
const ping = omw.host.recv();
omw.host.sendAgent(me, "pong");
const pong = omw.host.recv();
omw.host.unsubscribeAgent(sub);
omw.host.log("info", `${ping.kind}|${pong.kind}`);

// Two agents share this script. `whoami` tells each agent its own name, so it
// subscribes only to itself and plays a deterministic ping-pong with its own
// inbox. The handle goes to memory so a hot reload can reuse it, and is dropped
// explicitly to mirror the rust brain's RAII guard.
let me = omw::host::whoami();
let sub = omw::host::subscribe_agent(me);
omw::host::memory_set("sub", sub);
omw::host::send_agent(me, "ping");
let ping = omw::host::recv();
omw::host::send_agent(me, "pong");
let pong = omw::host::recv();
omw::host::unsubscribe_agent(sub);
omw::host::log("info", ping.kind + "|" + pong.kind);

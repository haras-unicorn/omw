// This brain never exits on its own: it subscribes to itself, sends one
// message, then loops on `recv` forever. The test's `outcome = "asserted"`
// stops it once the first three events have been observed.
const subscription = omw.host.subscribeAgent("alice");
omw.host.sendAgent("alice", "ping");
while (true) {
  const event = omw.host.recv();
  omw.host.log("info", event.kind);
}

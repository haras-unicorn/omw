// This brain never exits on its own: it subscribes to itself, sends one
// message, then loops on `recv` forever. The test's `outcome = "asserted"`
// stops it once the first three events have been observed.
let subscription = omw::host::subscribe_agent("alice");
omw::host::send_agent("alice", "ping");
loop {
  let event = omw::host::recv();
  omw::host::log("info", event.kind);
}

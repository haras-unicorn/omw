// Dev-only brain for eyeballing the tty live view (`dev test cli tty`): poll the
// inbox once a second so the TUI has something to render. It subscribes to
// lifecycle events (so Ctrl-C breaks the loop instead of spinning out the reload
// grace) and to the endpoint under `"model"` (so tenere gets a reply and does
// not hang). Forcing `listTools()` spawns the lazy mcp-nixos child, which is what
// puts the `mcp:nixos` tab on screen.
omw.host.subscribeLifecycle();
omw.host.subscribeEndpoint("model");

const nixos = omw.tooling.get("nixos");
omw.host.log("info", `nixos tools: ${nixos.listTools().length}`);

let tick = 0;
while (true) {
  omw.host.sleepFor(1000);
  const event = omw.host.tryRecv();
  if (event === undefined) {
    omw.host.log("info", `tick ${tick}: no events`);
  } else if (event.kind === "endpoint-message") {
    const session = event.payload.session;
    omw.host.streamEndpoint(session, { content: "test reply" });
    omw.host.streamEndpoint(session, { finishReason: "stop" });
    omw.host.log("info", `tick ${tick}: replied to ${session}`);
  } else if (event.kind === "shutdown" || event.kind === "reload") {
    omw.host.log("info", `tick ${tick}: ${event.kind}`);
    break;
  } else {
    omw.host.log("info", `tick ${tick}: ${JSON.stringify(event)}`);
  }
  tick += 1;
}

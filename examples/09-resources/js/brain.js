// 09-resources: subscribe to the resource list and to one resource, read the
// resource, and react to the mock's scripted, `after`-gated updates.
const t = omw.tooling.get("docs");

// Subscribe to the list first; the mock replays two scripted updates, each
// gated on a trace event, and the host pumps them into our inbox in order.
const subList = t.subscribeResourceList();
const _list0 = t.listResources();
const e1 = omw.host.recv();
const _list1 = t.listResources();
const e2 = omw.host.recv();

// Now subscribe to one resource; its content update is gated on our read.
const subNote = t.subscribeResource("mem://notes");
const _content1 = t.readResource("mem://notes");
const e3 = omw.host.recv();
const content2 = t.readResource("mem://notes");

t.unsubscribeResourceList(subList);
t.unsubscribeResource(subNote);
omw.host.log(
  "info",
  e1.kind + "|" + e2.kind + "|" + e3.kind + "|" + content2.content,
);

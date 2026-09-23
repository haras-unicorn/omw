// 09-resources: subscribe to the resource list and to one resource, read the
// resource, and react to the mock's scripted, `after`-gated updates.
let t = omw::tooling::get("docs");

// Subscribe to the list first; the mock replays two scripted updates, each
// gated on a trace event, and the host pumps them into our inbox in order.
let sub_list = t.subscribe_resource_list();
let _list0 = t.list_resources();
let e1 = omw::host::recv();
let _list1 = t.list_resources();
let e2 = omw::host::recv();

// Now subscribe to one resource; its content update is gated on our read.
let sub_note = t.subscribe_resource("mem://notes");
let _content1 = t.read_resource("mem://notes");
let e3 = omw::host::recv();
let content2 = t.read_resource("mem://notes");

t.unsubscribe_resource_list(sub_list);
t.unsubscribe_resource(sub_note);
omw::host::log(
  "info",
  e1.kind + "|" + e2.kind + "|" + e3.kind + "|" + content2.content,
);
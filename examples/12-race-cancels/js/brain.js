// 12-race-cancels: open two subscriptions, then cancel both in sequence. The
// `closed` observations are synchronous with each unsubscribe, so their order
// is fixed by the brain.
const t = omw.tooling.get("docs");
const list = t.subscribeResourceList();
const note = t.subscribeResource("mem://notes");
t.unsubscribeResourceList(list);
t.unsubscribeResource(note);
omw.host.log("info", "cancelled both");

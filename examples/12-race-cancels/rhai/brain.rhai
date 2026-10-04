// 12-race-cancels: open two subscriptions, then cancel both in sequence. The
// `closed` observations are synchronous with each unsubscribe, so their order
// is fixed by the brain.
let t = omw::tooling::get("docs");
let list = t.subscribe_resource_list();
let note = t.subscribe_resource("mem://notes");
t.unsubscribe_resource_list(list);
t.unsubscribe_resource(note);
omw::host::log("info", "cancelled both");

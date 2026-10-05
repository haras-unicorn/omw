# 12-race-cancels: open two subscriptions, then cancel both in sequence. The
# `closed` observations are synchronous with each unsubscribe, so their order
# is fixed by the brain.
t = omw.tooling.get("docs")
list_sub = t.subscribe_resource_list()
note = t.subscribe_resource("mem://notes")
t.unsubscribe_resource_list(list_sub)
t.unsubscribe_resource(note)
omw.host.log("info", "cancelled both")

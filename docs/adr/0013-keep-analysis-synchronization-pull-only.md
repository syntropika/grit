# Keep analysis synchronization pull-only

The automatic synchronization attempted by `next`, `plan`, `triage`, and `graph` will only pull remote changes; it never replays Pending mutations or otherwise writes to GitHub. Online mutation commands may apply their own requested write immediately, while offline writes remain queued until the user explicitly invokes Mutation reconciliation; the final CLI name for that operation remains undecided. This preserves the read-only contract of analysis commands even when an offline outbox exists.

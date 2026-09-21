# Exclude Issue deletion from v1

Hyfa v1 will not expose or queue GitHub Issue deletion even when the authenticated actor has permission to perform it. Users can close an Issue with an appropriate state reason, preserving discussion, references, and recoverability for the Local replica and graph. This deliberately omits a capability present in GitHub GraphQL because deletion is permanent and unusually difficult to reconcile safely; the restriction may be reviewed in a future version.

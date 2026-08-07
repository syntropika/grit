# Maintain a rebuildable local replica

Grit will analyze a rebuildable Local replica instead of refetching the repository for every command. GitHub remains authoritative: synchronization is the only bulk-ingestion path, mutations must succeed in GitHub before the replica changes, and an incomplete refresh never replaces the last valid replica; this makes `next`, `plan`, `triage`, and `graph` local and API-efficient while exposing the replica's `synced_at`. The replica's storage format remains undecided.

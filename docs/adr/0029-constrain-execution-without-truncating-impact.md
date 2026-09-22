# Constrain execution without truncating downstream impact

Execution scope combines ownership with explicit label filters and direct-child
membership, and constrains every simulated step while preserving the complete
Dependency graph and repository-wide unlocked-outcome scoring. This allows a
session to stay inside its authorized effort without inventing readiness by
removing outside prerequisites or changing the existing ranking policy's meaning.
Parent and child inventories are fetched on demand, retained in the validated
Local replica, and refreshed on subsequent synchronization independently of Issue
timestamps; missing inventory remains unknown, avoiding both a repository-wide
relationship scan on every first use and an unsafe assumption of empty membership.

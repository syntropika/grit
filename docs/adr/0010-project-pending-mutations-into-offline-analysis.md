# Project pending mutations into offline analysis

`next`, `plan`, `triage`, and `graph` will analyze a Working graph that deterministically overlays ordered Pending mutations on the Local replica. This lets disconnected assignments, state changes, and dependency changes affect readiness and availability immediately without modifying the synchronized base. Every affected result and machine-readable output must identify the provisional state as `pending`, because Mutation reconciliation may later reject it; the replica and its `synced_at` remain unchanged until GitHub accepts the mutations.

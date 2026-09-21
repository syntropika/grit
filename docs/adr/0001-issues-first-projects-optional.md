# Make Issues the foundation and Projects optional

Hyfa will construct and rank its graph directly from GitHub Issues and their native dependency relationships. GitHub Projects may later act as an optional selector or presentation layer, but the engine will not require them or treat Project-scoped fields as canonical; this avoids incomplete Project membership, conflicting per-Project values, and extra authentication requirements while retaining Projects as an integration path.

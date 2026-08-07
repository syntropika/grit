---
status: superseded by ADR-0021
---

# Discover the organization priority field

For an organization-owned Repository, Grit v1 will use an explicitly configured GitHub Issue Field ID as its Declared priority source when present. Without that configuration, it will automatically discover the organization-level single-select Issue Field whose name is exactly `Priority`. The explicit ID takes precedence and continues to identify the field if an organization renames it. This gives repositories using GitHub's default field a zero-configuration path while supporting customized organizations without making Projects or labels an implicit second source.

---
status: superseded by ADR-0028
---

# Propagate downstream priority through plans

Grit v1 will evaluate Declared priority on the outcomes a bounded plan advances or unlocks, not only on the Ready Issue that forms its first step. A lower-priority prerequisite may therefore outrank a higher-priority independent Issue when completing the prerequisite is the explainable route to higher-priority downstream work within the Planning horizon. This Priority propagation is calculated from the dependency simulation with its AND semantics and deduplicated outcomes; it never copies or persists a downstream label onto the prerequisite. Consequently, a Ready P3 Issue that unlocks a P0 outcome can rank above a Ready P1 Issue that advances no other work. P0 receives hard precedence only under ADR 0026's reachability condition; more distant ancestry remains a non-absolute structural signal.

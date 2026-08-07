---
status: superseded by ADR-0028
---

# Keep PageRank secondary to unlock value

`grit next` will not optimize PageRank. After readiness and availability gates, it will prefer the bounded plan whose first step produces the greatest explainable Unlock value within the Planning horizon, combined with Declared priority as specified by ADR 0018 and propagated through plan outcomes under ADR 0025. Critical priority has the bounded hard precedence defined by ADR 0026. PageRank remains a Structural centrality signal for diagnostics, tie-breaking, and graph visualization. This avoids treating topological prestige as scheduling value when dependency readiness is conjunctive and sequence-dependent.

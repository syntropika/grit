---
status: superseded by ADR-0028
---

# Treat P0 as a bounded expedite class

Grit v1 will treat P0 as a Critical priority rather than apply absolute lexicographic dominance to every priority tier. If at least one P0 Issue is Ready and Available, `grit next` will select a P0 Issue directly. Otherwise, a plan receives P0's hard precedence only when its simulated sequence makes a P0 Issue Ready within the Planning horizon; merely having a P0 descendant outside that reachable outcome is insufficient. A qualifying P0 plan outranks plans whose outcomes are exclusively P1–P4 or unspecified, regardless of their quantity, and competing qualifying plans use Critical distance under ADR 0027. P1–P4 remain ordinal signals within the Unlock profile, allowing a larger Unlock set to compensate for their differences instead of reducing Grit to a label sorter.

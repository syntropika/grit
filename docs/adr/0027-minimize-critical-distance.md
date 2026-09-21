---
status: superseded by ADR-0028
---

# Minimize critical distance

When no P0 Issue is already Ready and Available but more than one bounded plan can make a P0 Ready, Hyfa v1 will prefer the plan with the smallest Critical distance. The distance is the minimum number of simulated Issue completions required from the current state, including every prerequisite required by conjunctive dependency semantics rather than the edge length of one convenient path. Because v1 consumes no effort estimate, every completed Issue contributes one unit. A P0 that cannot become Ready within the Planning horizon does not participate in this hard comparison. This favors the fastest explainable route to executable critical work without pretending to know task durations.

# Make readiness a hard gate for next-work recommendations

`grit next` will rank only Ready Issues: no priority, impact, or graph-centrality score may compensate for an unsatisfied or unknown blocker. `grit triage` may still highlight important blocked Issues and explain what prevents them from becoming ready, keeping recommendations executable while preserving visibility into critical blocked work.

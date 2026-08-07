# Exclude closed Issues from the operational graph

Grit will calculate readiness, ranking, and current-work recommendations from an Operational graph containing only open Issues and unsatisfied Dependencies. Closed Issues satisfy their Dependencies but do not participate in operational metrics; historical analysis will use a separate explicit view, preventing old topology from changing present recommendations at the cost of not carrying historical centrality into the active ranking.

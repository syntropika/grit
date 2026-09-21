# Effort, impact, and priority in Beads

Checked on August 6, 2026 against:

- `gastownhall/beads` at `4ebc98927cca05198c68fd04a42c979cf48207e9`.
- `Dicklesworthstone/beads_viewer` at `fba4591a4b1553a988a7cf71e631c1fd570b1b32`.

## Short answer

Beads core does have an effort estimate, although it is not called `effort`: it stores the optional `estimated_minutes` field. It also stores a human-assigned P0–P4 `priority`. It has no native `impact` field or graph-based impact ranking.

Beads Viewer does not load a declared `impact` either. It calculates several different things that it calls “impact”: a composite score, chain depth, and hypothetical unblocking deltas. Therefore, copying the name without separating its meanings would introduce ambiguity into Hyfa.

## What Beads core persists

| Concept | Native representation | Use |
|---|---|---|
| Priority | `priority: int`, P0–P4; P0 is critical | `bd create` uses P2 by default and `bd ready` sorts by priority by default. |
| Effort | `estimated_minutes: *int`, optional and non-negative | `bd create/update --estimate N`; expresses expected time, not value. |
| Impact | Does not exist as a native field | It could be stored as integration metadata, but Beads assigns it no semantics and does not use it for sorting. |

The canonical model lists `Priority` and `EstimatedMinutes`, with no `Impact` field ([`Issue` model](https://github.com/gastownhall/beads/blob/4ebc98927cca05198c68fd04a42c979cf48207e9/internal/types/types.go#L17-L40)); the schema documentation describes them as priority 0–4 and an optional time estimate ([Issue Schema](https://github.com/gastownhall/beads/blob/4ebc98927cca05198c68fd04a42c979cf48207e9/docs/architecture/index.md#L57-L80)). The persisted table confirms the `priority` and `estimated_minutes` columns and does not define `impact` ([initial migration](https://github.com/gastownhall/beads/blob/4ebc98927cca05198c68fd04a42c979cf48207e9/internal/storage/schema/migrations/0001_create_issues.up.sql#L1-L20)).

The CLI exposes `--estimate` on creation and update ([`bd create`](https://github.com/gastownhall/beads/blob/4ebc98927cca05198c68fd04a42c979cf48207e9/cmd/bd/create.go#L840-L865), [`bd update`](https://github.com/gastownhall/beads/blob/4ebc98927cca05198c68fd04a42c979cf48207e9/cmd/bd/update.go#L934-L950)). `bd ready` offers `priority`, `hybrid`, and `oldest`, with `priority` as the default policy ([`bd ready` flags](https://github.com/gastownhall/beads/blob/4ebc98927cca05198c68fd04a42c979cf48207e9/cmd/bd/ready.go#L676-L683)); its implementation sorts P0 before P1, and so on ([ready work ordering](https://github.com/gastownhall/beads/blob/4ebc98927cca05198c68fd04a42c979cf48207e9/internal/storage/sqlbuild/ready.go#L47-L68)). In other words, Beads core filters by readiness and sorts primarily by priority/date; it does not calculate PageRank, betweenness, or an impact score.

Beads does support arbitrary JSON in `metadata` as an extension, but the documentation itself distinguishes that information from core fields and leaves policy to the integration ([Issue Metadata](https://github.com/gastownhall/beads/blob/4ebc98927cca05198c68fd04a42c979cf48207e9/docs/core-concepts/metadata.md#L6-L11)). Storing impact there would be an external convention, not a Beads ranking capability.

## What Beads Viewer does

### Schema read

The model loaded by Viewer retains `Priority` and `EstimatedMinutes`, but does not contain `Impact` ([`model.Issue`](https://github.com/Dicklesworthstone/beads_viewer/blob/fba4591a4b1553a988a7cf71e631c1fd570b1b32/pkg/model/types.go#L10-L36)). Therefore, its impacts are derived metrics, not metadata declared by the person who creates the task.

### Composite ranking called `ImpactScore`

In the current code, `ImpactScore` combines PageRank (22%), betweenness (20%), direct blocks (13%), age (5%), explicit priority (10%), time-to-impact (10%), urgency (10%), and risk (10%) ([structure and weights](https://github.com/Dicklesworthstone/beads_viewer/blob/fba4591a4b1553a988a7cf71e631c1fd570b1b32/pkg/analysis/priority.go#L13-L69), [calculation](https://github.com/Dicklesworthstone/beads_viewer/blob/fba4591a4b1553a988a7cf71e631c1fd570b1b32/pkg/analysis/priority.go#L123-L214)).

`estimated_minutes` only enters the `time_to_impact` component: it uses the explicit estimate or the median and rewards shorter times together with greater chain depth ([time-to-impact calculation](https://github.com/Dicklesworthstone/beads_viewer/blob/fba4591a4b1553a988a7cf71e631c1fd570b1b32/pkg/analysis/priority.go#L303-L368)). It is not a clean `benefit / effort` model: cost does not appear as a separate dimension of the score.

There is also a documentation discrepancy: the README still publishes an earlier 30/30/20/10/10 formula ([README](https://github.com/Dicklesworthstone/beads_viewer/blob/fba4591a4b1553a988a7cf71e631c1fd570b1b32/README.md#L1106-L1129)), while the code uses the eight components above. To reproduce Viewer, the pinned version of the code should be used, not that README formula.

### Other meanings of “impact” in Viewer

- **Impact Depth / Keystone**: maximum downstream chain length, `1 + max(...)`; it is neither business value nor the total amount unlocked ([metric description](https://github.com/Dicklesworthstone/beads_viewer/blob/fba4591a4b1553a988a7cf71e631c1fd570b1b32/pkg/ui/insights.go#L42-L60), [implementation](https://github.com/Dicklesworthstone/beads_viewer/blob/fba4591a4b1553a988a7cf71e631c1fd570b1b32/pkg/analysis/graph.go#L2059-L2080)).
- **What-if impact**: simulates completing an Issue and reports direct and transitive unlocks, reduction in blocked Issues, reduction in depth, estimated days, and parallelization gain ([model](https://github.com/Dicklesworthstone/beads_viewer/blob/fba4591a4b1553a988a7cf71e631c1fd570b1b32/pkg/analysis/priority.go#L448-L466), [calculation](https://github.com/Dicklesworthstone/beads_viewer/blob/fba4591a4b1553a988a7cf71e631c1fd570b1b32/pkg/analysis/priority.go#L779-L837)).
- **`high-impact` recipe**: simply means “highest PageRank,” not the composite score ([recipes](https://github.com/Dicklesworthstone/beads_viewer/blob/fba4591a4b1553a988a7cf71e631c1fd570b1b32/README.md#L1074-L1085)).
- **Quick wins**: although the code describes them as balancing impact and effort, its simplicity heuristic uses the blocking ratio and depth, not `estimated_minutes` ([quick wins](https://github.com/Dicklesworthstone/beads_viewer/blob/fba4591a4b1553a988a7cf71e631c1fd570b1b32/pkg/analysis/triage.go#L864-L900)). Viewer does use `estimated_minutes` for ETA/capacity ([ETA](https://github.com/Dicklesworthstone/beads_viewer/blob/fba4591a4b1553a988a7cf71e631c1fd570b1b32/pkg/analysis/eta.go#L12-L31)), but its quick-win selection is not a rigorous effort analysis.

## Predecisional vocabulary considered for Hyfa

This section predates ADR 0028 and is retained as research context. The normative v1 vocabulary is **Declared priority**, **Unlock set**, and **Unlock profile**. Estimated effort and declared or business value are outside the v1 ranking contract.

Four terms should be kept separate:

1. **Declared priority**: human-assigned order of attention—for example, P0–P4—that can encapsulate urgency, commitment, and the team's judgment.
2. **Estimated effort**: expected cost to complete the Issue, expressed in a declared unit. It is human-provided data; it should not be inferred from the number of dependencies.
3. **Declared value**: benefit of the outcome itself—users affected, risk mitigated, contractual or strategic value—regardless of the Issue's position in the graph.
4. **Unblocking value**: calculated effect on the plan: how much work becomes ready or moves closer to ready within the horizon. Fan-out, PageRank, and path/depth are related structural signals, but do not replace declared value.

Example: fixing data loss can have very high declared value even if it does not unblock any other Issue. In contrast, an internal migration can have little direct value but high unblocking value because it enables seven tasks. Calling them both `impact` makes the ranking difficult to explain clearly.

### Historical recommendation (superseded for v1)

- Do not copy Viewer's ambiguous `ImpactScore` term.
- Use **unblocking value** for the graph-derived signal and keep PageRank/centrality as separate structural metrics.
- If human-assigned benefit is to be captured, call it **declared value** (or `business_value`), make it optional, and do not invent it when it is missing.
- Treat **estimated effort** as a separate cost. If no reliable field is available, leave it unknown instead of equating “few dependencies” with “little effort.”
- In an initial Issues-only version, human-assigned priority + unblocking value may be sufficient. `business_value` and effort can be incorporated when there is an explicit convention or a Project that provides them.

# Static graph browser benchmark

**Measured:** 2026-08-07  
**Implementation:** Issue #17  
**Browser:** Google Chrome 150.0.7871.186, headless  
**Host:** Linux 7.0.0-28-generic, x86-64 QEMU virtual CPU, 12 cores, 62 GiB RAM

## Decision

Grit renders the complete network when an artifact has at most 5,000 nodes and
20,000 edges. Crossing either limit opens a constrained view of at most 500
nodes. The binary seeds that view from Ready Issues and their undirected
Dependency neighborhoods in Stable node key order. The visitor can restore the
initial overview, select any result from the complete accessible table to open
its bounded neighborhood, or explicitly request the complete network.

The 5,000/20,000 boundary is the largest full-network case measured here. Its
median time to interactive was 1.31 seconds, including the complete static
table, and its SVG construction took 94.2 ms. At 10,000/40,000, constraining
the initial SVG reduced browser render work to 10.5 ms. HTML parsing and the
complete accessible table still dominate that larger case, so it is treated as
a usable constrained mode rather than an expansion of the validated full range.

## Median results

Each row is the median of five release-mode runs over the same deterministic
synthetic DAG. Sizes are decimal megabytes. `Layout` is the binary's repeated
precomputed Dependency-layer pass. `Load` is navigation start through execution
of the deferred application script. `Render` is initial SVG DOM construction.
`TTI` includes binding controls, applying the initial search state, and a forced
style/layout read. Browser-side layout and ranking are both absent.

| Nodes | Edges | Mode | Artifact | Complete site | Binary layout | Browser load | SVG render | TTI |
|---:|---:|---|---:|---:|---:|---:|---:|---:|
| 100 | 400 | full | 0.14 MB | 0.32 MB | 0.11 ms | 54.9 ms | 3.3 ms | 68.5 ms |
| 1,000 | 4,000 | full | 1.36 MB | 2.91 MB | 1.40 ms | 188.4 ms | 21.1 ms | 245.6 ms |
| 5,000 | 20,000 | full | 6.87 MB | 14.60 MB | 6.67 ms | 1,007.1 ms | 94.2 ms | 1,306.9 ms |
| 10,000 | 40,000 | constrained | 13.76 MB | 29.22 MB | 14.62 ms | 1,989.3 ms | 10.5 ms | 2,472.2 ms |

For pipeline context, median release-mode artifact construction at the four
sizes was 1.04, 10.05, 63.77, and 179.28 ms. Median JSON serialization was
0.22, 1.56, 7.00, and 11.76 ms; median static HTML rendering was 0.59, 6.16,
32.49, and 65.86 ms. These stages are recorded separately so a later regression
cannot be hidden inside one end-to-end number.

## Reproduce

Run from the repository root with a Chrome-compatible browser:

```bash
GRIT_BROWSER=google-chrome cargo test --release \
  graph::benchmark::dense_graph_browser_benchmark \
  -- --ignored --exact --nocapture
```

Repeat the command five times and take the median for each reported field to
match the table above.

The ignored benchmark builds normalized synthetic Issues and Dependencies,
runs Grit's real artifact builder and layered layout, renders the production
HTML/CSS/JavaScript, and opens it with browser network egress disabled. It emits
one JSON record per size with artifact bytes, complete-site bytes, artifact
construction, layout, serialization, HTML generation, load, SVG render, TTI,
selected network mode, and initially rendered node and edge counts.

## Interpretation limits

These numbers establish a product guardrail on the stated reference host; they
are not a universal browser SLA. Real titles and labels change byte size, and
client hardware varies. The benchmark deliberately keeps the full accessible
table in constrained mode. If future measurements show that table parsing is
the limiting factor, virtualization must preserve an equivalent keyboard and
screen-reader path rather than silently removing rows.

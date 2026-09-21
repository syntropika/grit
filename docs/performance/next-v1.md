# `next/v1` Repository-scale benchmark

This benchmark is the performance acceptance gate for local ranking. It does not measure GitHub authentication, HTTP requests, Synchronization, or deterministic fixture construction.

## Reference environment

- Architecture: `x86_64`
- CPU: 12-core QEMU Virtual CPU 2.5+ under KVM, one thread per core
- Cache reported by the guest: 384 KiB L1d, 48 MiB L2, 16 MiB L3
- Rust: `rustc 1.94.0 (4a4ef493e 2026-03-02)`, LLVM 21.1.8
- Build: Cargo `--release`, default features

The test is single-process and the ranking engine itself is single-threaded. Host load can affect elapsed time, so deterministic work counts and byte-identical cold/warm output are checked independently of timing.

## Fixture

The deterministic replica contains exactly 5,000 Issues and 20,000 distinct internal Dependencies. Its Operational graph has 800 initially Executable Issues and 4,000 blocked Issues, plus 200 closed roots that exercise satisfied history. Ten blocked Issues have three open and two satisfied AND blockers, so their closures fit the three-step horizon and produce realizable cascades; the remaining outcomes have five open blockers and are infeasible within the horizon. Priorities cycle through P1, P2, P3, and P4, with one infeasible P0 outcome exercising critical-route rejection without replacing the normal frontier. The Planning horizon is three.

This shape exercises graph preparation, SCC detection, the 20-iteration PageRank stage, the full normative shortlist/probe/beam policy, deterministic truncation, cache publication and rehydration, output assembly, and JSON serialization. It is not presented as a universal worst case.

## Reproduce

```bash
cargo test --release \
  ranking::performance_tests::repository_scale_profile_meets_the_next_v1_latency_budget \
  -- --ignored --nocapture
```

The ignored test fails when cold local ranking plus serialization reaches one second or warm local ranking plus serialization reaches 250 milliseconds. It also fails if cold and warm serialized analyses differ.

For an existing repository, `grit next --repo OWNER/REPO --profile --json` exposes the same phase boundaries. `performance.analysis_serialization` measures the flattened `NextAnalysis` document rather than claiming to time the self-describing outer profile envelope. Its `performance.synchronization_included` field is always `false`. Human profiling does not perform an otherwise unused JSON serialization.

## Results

Five consecutive release runs on 2026-08-07 produced:

| Measurement | Median | Observed range |
| --- | ---: | ---: |
| Cold total | 662.2 ms | 647.7–671.5 ms |
| Cold bounded search | 635.2 ms | 621.8–645.8 ms |
| Warm total | 45.4 ms | 44.9–46.9 ms |

A representative cold run separated the remaining stages as follows:

| Phase | Time |
| --- | ---: |
| Graph preparation | 5.5 ms |
| SCC detection | 5.2 ms |
| Readiness | 1.3 ms |
| PageRank | 2.2 ms |
| Bounded search | 635.2 ms |
| Output assembly | 0.7 ms |
| Cache publication | about 14 ms |
| Analysis JSON serialization | <0.1 ms |

The corresponding median warm run spent about 36.5 ms loading, verifying, replaying, and rehydrating the cache, 3.7 ms in graph preparation, 3.3 ms in SCC detection, 1.2 ms in readiness, and 0.6 ms in output assembly. PageRank and bounded search were not recomputed.

Both paths reported the same recommendation, alternatives, hashes, truncation reasons, and deterministic work counts: 4,192 main-search successors and 23,552 probe successors.

## Cache identity and safety

The cache key covers:

- the Local replica snapshot hash;
- the ordered Pending-operation content hashes in the Working graph;
- the normalized Execution scope;
- `next/v1` policy version;
- Planning horizon, alternative limit, and state budget;
- PageRank damping, iteration count, and bucket scale.

The cache is a rebuildable optimization stored beside the Local replica. A missing, malformed, stale, or incompatible entry becomes a cache miss. Cache publication uses the same write, file sync, atomic rename, and directory sync discipline as replica publication. GitHub and the validated Local replica remain the sources of input truth.

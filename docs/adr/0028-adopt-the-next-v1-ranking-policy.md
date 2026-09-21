---
status: accepted
---

# Adopt the weight-free bounded `next/v1` ranking policy

Hyfa v1 will rank only executable work using the versioned, lexicographic policy specified in `docs/design/ranking-next-v1.md`: bounded P0 expedite rules first, then the count, priority composition, and timing of distinct newly Ready outcomes, followed only by step priority, PageRank, and a Stable node key. This deliberately rejects an opaque weighted score and unbounded optimization; it preserves ordinal Priority, allows a larger Unlock set to compensate for differences among P1–P4 priorities, keeps P0 exceptional only when executable or reachable within the horizon, and reports every search limit instead of claiming global optimality. `plan` will expose the one-lane rollout, current parallel frontier, and structural dependency layers in v1, but will not accept worker capacity until a separate policy can model joint selection and critical rounds honestly.

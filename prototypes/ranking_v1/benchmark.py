#!/usr/bin/env python3
"""PROTOTYPE benchmark for bounded `next/v1` search on synthetic DAGs."""

from __future__ import annotations

from dataclasses import dataclass
from random import Random
from time import perf_counter


@dataclass(frozen=True)
class Node:
    number: int
    blockers: frozenset[int]
    priority: int


@dataclass(frozen=True)
class State:
    sequence: tuple[int, ...]
    completed: frozenset[int]
    unlocked: frozenset[int]
    curve: tuple[int, ...]


def synthetic_dag(size: int, blockers_per_node: int = 4) -> dict[int, Node]:
    rng = Random(7)
    root_count = max(16, size // 20)
    nodes: dict[int, Node] = {}
    for number in range(size):
        if number < root_count:
            blockers = frozenset()
        else:
            available = range(max(0, number - 256), number)
            count = min(1 + rng.randrange(blockers_per_node), len(available))
            blockers = frozenset(rng.sample(list(available), count))
        nodes[number] = Node(number, blockers, rng.randrange(1, 5))
    return nodes


def dependents(nodes: dict[int, Node]) -> dict[int, tuple[int, ...]]:
    result: dict[int, list[int]] = {number: [] for number in nodes}
    for node in nodes.values():
        for blocker in node.blockers:
            result[blocker].append(node.number)
    return {number: tuple(values) for number, values in result.items()}


def pagerank(nodes: dict[int, Node], iterations: int = 20) -> dict[int, float]:
    count = len(nodes)
    scores = {number: 1.0 / count for number in nodes}
    damping = 0.85
    for _ in range(iterations):
        dangling = sum(scores[n] for n, node in nodes.items() if not node.blockers)
        base = (1.0 - damping) / count + damping * dangling / count
        next_scores = {number: base for number in nodes}
        for number, node in nodes.items():
            if not node.blockers:
                continue
            share = damping * scores[number] / len(node.blockers)
            for blocker in node.blockers:
                next_scores[blocker] += share
        scores = next_scores
    return scores


def ready(
    nodes: dict[int, Node],
    reverse: dict[int, tuple[int, ...]],
    roots: frozenset[int],
    completed: frozenset[int],
) -> set[int]:
    candidates = set(roots)
    for blocker in completed:
        candidates.update(reverse[blocker])
    return {
        number
        for number in candidates
        if number not in completed and nodes[number].blockers <= completed
    }


def relaxed_reach(
    reverse: dict[int, tuple[int, ...]],
    seeds: set[int],
    depth: int,
    excluded: frozenset[int] | set[int],
) -> set[int]:
    """Optimistic structural reach used only to keep delayed cascades in the beam."""
    reached: set[int] = set()
    frontier = set(seeds)
    for _ in range(depth):
        following: set[int] = set()
        for number in frontier:
            following.update(reverse[number])
        following.difference_update(excluded)
        following.difference_update(reached)
        reached.update(following)
        frontier = following
        if not frontier:
            break
    return reached


def alternating_union(
    ranked_groups: tuple[tuple[State, ...], ...],
    quotas: tuple[int, ...],
    limit: int,
) -> list[State]:
    selected: list[State] = []
    seen: set[tuple[int, ...]] = set()

    def add(state: State) -> None:
        if state.sequence not in seen and len(selected) < limit:
            selected.append(state)
            seen.add(state.sequence)

    for ranked, quota in zip(ranked_groups, quotas, strict=True):
        for state in ranked[:quota]:
            add(state)

    offset = 0
    while len(selected) < limit:
        added = False
        for ranked in ranked_groups:
            if offset < len(ranked):
                before = len(selected)
                add(ranked[offset])
                added = added or len(selected) != before
        if not added and all(offset >= len(ranked) - 1 for ranked in ranked_groups):
            break
        offset += 1
    return selected


def search(
    nodes: dict[int, Node],
    horizon: int = 3,
    first_step_width: int = 96,
    beam_width: int = 64,
    branch_width: int = 32,
) -> tuple[State, int]:
    reverse = dependents(nodes)
    roots = frozenset(number for number, node in nodes.items() if not node.blockers)
    initial_ready = ready(nodes, reverse, roots, frozenset())
    beam = [State((), frozenset(), frozenset(), ())]
    expanded = 0

    def state_key(state: State) -> tuple[object, ...]:
        profile = tuple(
            sum(nodes[number].priority == priority for number in state.unlocked)
            for priority in (1, 2, 3, 4)
        )
        step_priorities = tuple(-nodes[number].priority for number in state.sequence)
        return (
            len(state.unlocked),
            profile,
            state.curve,
            step_priorities,
            -state.sequence[0],
        )

    def project(state: State, number: int) -> State:
        completed = state.completed | {number}
        newly_ready = {
            dependent
            for dependent in reverse[number]
            if dependent not in completed and nodes[dependent].blockers <= completed
        }
        unlocked = state.unlocked | (newly_ready - initial_ready)
        return State(
            state.sequence + (number,),
            completed,
            frozenset(unlocked),
            state.curve + (len(unlocked),),
        )

    def potential_key(state: State, remaining_steps: int) -> tuple[object, ...]:
        if remaining_steps <= 0:
            return state_key(state)
        seeds = set(state.unlocked - state.completed)
        relaxed = relaxed_reach(
            reverse,
            seeds,
            remaining_steps,
            state.completed | state.unlocked | initial_ready,
        )
        return (len(state.unlocked) + len(relaxed), state_key(state))

    for depth in range(horizon):
        next_states: list[State] = []
        for state in beam:
            frontier = ready(nodes, reverse, roots, state.completed)

            projected = tuple(project(state, number) for number in frontier)
            by_realized = tuple(sorted(projected, key=state_key, reverse=True))
            by_potential = tuple(
                sorted(
                    projected,
                    key=lambda candidate: potential_key(
                        candidate, horizon - len(candidate.sequence)
                    ),
                    reverse=True,
                )
            )
            expansion_width = first_step_width if depth == 0 else branch_width
            realized_quota = 32 if depth == 0 else 16
            potential_quota = 32 if depth == 0 else 16
            selected = alternating_union(
                (by_realized, by_potential),
                (realized_quota, potential_quota),
                expansion_width,
            )
            for candidate in selected:
                next_states.append(candidate)
                expanded += 1

        by_realized = tuple(sorted(next_states, key=state_key, reverse=True))
        by_potential = tuple(
            sorted(
                next_states,
                key=lambda state: potential_key(state, horizon - len(state.sequence)),
                reverse=True,
            )
        )
        beam = alternating_union(
            (by_realized, by_potential), (beam_width // 2, beam_width // 2), beam_width
        )
        if not beam:
            break

    return max(beam, key=state_key), expanded


def delayed_cascade() -> dict[int, Node]:
    nodes = {number: Node(number, frozenset(), 2) for number in range(66)}
    intermediate = 66
    nodes[intermediate] = Node(intermediate, frozenset({0}), 2)
    next_number = 67
    for _ in range(100):
        nodes[next_number] = Node(next_number, frozenset({intermediate}), 2)
        next_number += 1
    for root in range(1, 66):
        for _ in range(2):
            nodes[next_number] = Node(next_number, frozenset({root}), 2)
            next_number += 1
    return nodes


def main() -> None:
    print("PROTOTYPE — bounded search, Python reference only")
    cascade_winner, cascade_expanded = search(delayed_cascade(), horizon=2)
    cascade_verdict = "EXPECTED" if cascade_winner.sequence[0] == 0 else "SURPRISING"
    print(
        f"delayed-cascade {cascade_verdict} first={cascade_winner.sequence[0]} "
        f"unlocks={len(cascade_winner.unlocked)} states={cascade_expanded}"
    )
    if cascade_verdict != "EXPECTED":
        raise SystemExit(1)
    for size, blocker_cap in ((100, 4), (1_000, 4), (5_000, 8)):
        started = perf_counter()
        nodes = synthetic_dag(size, blocker_cap)
        built = perf_counter()
        winner, expanded = search(nodes)
        searched = perf_counter()
        pagerank(nodes)
        finished = perf_counter()
        edges = sum(len(node.blockers) for node in nodes.values())
        print(
            f"V={size:>5} E={edges:>6} build={(built-started)*1000:>7.1f}ms "
            f"search={(searched-built)*1000:>7.1f}ms pagerank={(finished-searched)*1000:>7.1f}ms "
            f"states={expanded:>5} "
            f"first={winner.sequence[0]} unlocks={len(winner.unlocked)}"
        )


if __name__ == "__main__":
    main()

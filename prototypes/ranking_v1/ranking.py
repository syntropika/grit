"""PROTOTYPE: pure bounded-plan ranking logic for the `next/v1` policy."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass, field


@dataclass(frozen=True)
class Issue:
    number: int
    priority: str | None = None
    blockers: frozenset[int] = field(default_factory=frozenset)
    assigned: bool = False


@dataclass(frozen=True)
class Scenario:
    name: str
    question: str
    issues: tuple[Issue, ...]
    horizon: int
    expected: int | None


@dataclass(frozen=True)
class PlanResult:
    first: int
    sequence: tuple[int, ...]
    mode: str
    critical_distance: int | None
    p0_curve: tuple[int, ...]
    unlocks: frozenset[int]
    unlock_curve: tuple[int, ...]
    priority_profile: tuple[int, int, int, int]
    step_priorities: tuple[int, ...]


def priority_rank(priority: str | None) -> int:
    return {
        "p0": 5,
        "p1": 4,
        "p2": 3,
        None: 3,
        "conflict": 3,
        "p3": 2,
        "p4": 1,
    }[priority]


def ready_numbers(issues: dict[int, Issue], open_numbers: frozenset[int]) -> frozenset[int]:
    known_numbers = frozenset(issues)
    return frozenset(
        number
        for number in open_numbers
        if not (issues[number].blockers & open_numbers)
        and not (issues[number].blockers - known_numbers)
    )


def executable_numbers(
    issues: dict[int, Issue], open_numbers: frozenset[int]
) -> tuple[int, ...]:
    return tuple(
        sorted(
            number
            for number in ready_numbers(issues, open_numbers)
            if not issues[number].assigned
        )
    )


def prerequisite_closure(
    issues: dict[int, Issue], target: int, open_numbers: frozenset[int]
) -> frozenset[int] | None:
    closure: set[int] = set()
    visiting: set[int] = set()

    def visit(number: int) -> bool:
        if number in visiting:
            return False
        visiting.add(number)
        for blocker in issues[number].blockers:
            if blocker not in issues:
                return False
            if blocker not in open_numbers:
                continue
            if blocker == target:
                return False
            closure.add(blocker)
            if not visit(blocker):
                return False
        visiting.remove(number)
        return True

    return frozenset(closure) if visit(target) else None


def policy_steps(
    issues: dict[int, Issue], open_numbers: frozenset[int], remaining: int
) -> tuple[int, ...]:
    executable = executable_numbers(issues, open_numbers)
    ready_p0 = tuple(
        number for number in executable if issues[number].priority == "p0"
    )
    if ready_p0:
        return ready_p0

    critical_first_steps: set[int] = set()
    for number in open_numbers:
        if issues[number].priority != "p0":
            continue
        closure = prerequisite_closure(issues, number, open_numbers)
        if closure is None or len(closure) > remaining:
            continue
        if any(issues[step].assigned for step in closure):
            continue
        critical_first_steps.update(closure & set(executable))

    return tuple(sorted(critical_first_steps)) if critical_first_steps else executable


def enumerate_plans(scenario: Scenario) -> tuple[tuple[int, ...], ...]:
    issues = {issue.number: issue for issue in scenario.issues}
    initial_open = frozenset(issues)
    plans: list[tuple[int, ...]] = []

    def visit(open_numbers: frozenset[int], sequence: tuple[int, ...]) -> None:
        if len(sequence) == scenario.horizon:
            plans.append(sequence)
            return
        executable = policy_steps(
            issues, open_numbers, scenario.horizon - len(sequence)
        )
        if not executable:
            plans.append(sequence)
            return
        for number in executable:
            visit(open_numbers - {number}, sequence + (number,))

    visit(initial_open, ())
    return tuple(plan for plan in plans if plan)


def simulate(scenario: Scenario, sequence: tuple[int, ...]) -> PlanResult:
    issues = {issue.number: issue for issue in scenario.issues}
    open_numbers = frozenset(issues)
    initial_ready = ready_numbers(issues, open_numbers)
    seen_ready = set(initial_ready)
    unlocks: set[int] = set()
    unlock_curve: list[int] = []
    p0_curve: list[int] = []
    first_p0_step: int | None = None
    p0_ready: set[int] = set()

    for step, number in enumerate(sequence, start=1):
        before = ready_numbers(issues, open_numbers)
        allowed = policy_steps(
            issues, open_numbers, scenario.horizon - step + 1
        )
        if number not in before or number not in allowed:
            raise ValueError(f"non-executable plan step #{number}")
        open_numbers = open_numbers - {number}
        after = ready_numbers(issues, open_numbers)
        newly_ready = after - seen_ready
        seen_ready.update(newly_ready)
        unlocks.update(newly_ready)
        new_p0 = {n for n in newly_ready if issues[n].priority == "p0"}
        if new_p0 and first_p0_step is None:
            first_p0_step = step
        p0_ready.update(new_p0)
        unlock_curve.append(len(unlocks))
        p0_curve.append(len(p0_ready))

    while len(unlock_curve) < scenario.horizon:
        unlock_curve.append(len(unlocks))
        p0_curve.append(len(p0_ready))

    profile = (
        sum(issues[n].priority == "p1" for n in unlocks),
        sum(issues[n].priority in {"p2", None, "conflict"} for n in unlocks),
        sum(issues[n].priority == "p3" for n in unlocks),
        sum(issues[n].priority == "p4" for n in unlocks),
    )
    initially_ready_p0 = {
        n
        for n in initial_ready
        if issues[n].priority == "p0" and not issues[n].assigned
    }
    mode = "p0-ready" if initially_ready_p0 else ("p0-path" if p0_ready else "normal")
    return PlanResult(
        first=sequence[0],
        sequence=sequence,
        mode=mode,
        critical_distance=0 if mode == "p0-ready" else first_p0_step,
        p0_curve=tuple(p0_curve),
        unlocks=frozenset(unlocks),
        unlock_curve=tuple(unlock_curve),
        priority_profile=profile,
        step_priorities=(
            tuple(priority_rank(issues[number].priority) for number in sequence)
            + (0,) * (scenario.horizon - len(sequence))
        ),
    )


def normal_key(result: PlanResult) -> tuple[object, ...]:
    return (
        len(result.unlocks),
        result.priority_profile,
        result.unlock_curve,
        result.step_priorities,
        -result.first,
        tuple(-number for number in result.sequence),
    )


def best_distinct_first(
    results: list[PlanResult], key: Callable[[PlanResult], tuple[object, ...]]
) -> tuple[PlanResult, ...]:
    best: dict[int, PlanResult] = {}
    for result in results:
        previous = best.get(result.first)
        if previous is None or key(result) > key(previous):
            best[result.first] = result
    return tuple(sorted(best.values(), key=key, reverse=True))


def rank(scenario: Scenario) -> tuple[PlanResult, ...]:
    issues = {issue.number: issue for issue in scenario.issues}
    initial_open = frozenset(issues)
    initial_ready_p0 = {
        n
        for n in ready_numbers(issues, initial_open)
        if issues[n].priority == "p0" and not issues[n].assigned
    }
    results = [simulate(scenario, sequence) for sequence in enumerate_plans(scenario)]

    if initial_ready_p0:
        results = [result for result in results if result.first in initial_ready_p0]
        return best_distinct_first(
            results, lambda result: (result.p0_curve, normal_key(result))
        )

    critical = [result for result in results if result.mode == "p0-path"]
    if critical:
        minimum_distance = min(result.critical_distance or 10**9 for result in critical)
        critical = [
            result for result in critical if result.critical_distance == minimum_distance
        ]
        return best_distinct_first(
            critical, lambda result: (result.p0_curve, normal_key(result))
        )

    return best_distinct_first(results, normal_key)


def scenarios() -> tuple[Scenario, ...]:
    chain = [Issue(1, "p1"), Issue(2, "p3"), Issue(3, blockers=frozenset({2}))]
    chain.extend(Issue(number, blockers=frozenset({3})) for number in range(4, 11))

    p0_vs_bulk = [
        Issue(1),
        Issue(2),
        Issue(20, "p0", frozenset({1})),
    ]
    p0_vs_bulk.extend(
        Issue(number, "p1", frozenset({2})) for number in range(21, 31)
    )

    distant_p0 = [
        Issue(1),
        Issue(2),
        Issue(3, blockers=frozenset({1})),
        Issue(4, blockers=frozenset({3})),
        Issue(5, blockers=frozenset({4})),
        Issue(6, "p0", frozenset({5})),
    ]
    distant_p0.extend(Issue(number, blockers=frozenset({2})) for number in range(20, 24))

    count_beats_tier = [Issue(1, "p4"), Issue(2, "p1")]
    count_beats_tier.extend(
        Issue(number, "p4", frozenset({1})) for number in range(10, 13)
    )
    count_beats_tier.extend(
        Issue(number, "p1", frozenset({2})) for number in range(20, 22)
    )

    return (
        Scenario(
            "lookahead-chain",
            "A P3 prerequisite should beat an independent P1 when step two unlocks seven Issues.",
            tuple(chain),
            2,
            2,
        ),
        Scenario(
            "and-complementarity",
            "A+B must both complete before C becomes Ready; either executable first step is valid.",
            (
                Issue(1),
                Issue(2),
                Issue(3, blockers=frozenset({1, 2})),
            ),
            2,
            1,
        ),
        Scenario(
            "diamond-deduplication",
            "A shared downstream Issue should enter the Unlock set once after both branches complete.",
            (
                Issue(1),
                Issue(2, "p1"),
                Issue(3, blockers=frozenset({1})),
                Issue(4, blockers=frozenset({1})),
                Issue(5, blockers=frozenset({3, 4})),
            ),
            3,
            1,
        ),
        Scenario(
            "p0-expedite",
            "One reachable P0 should beat a bulk P1 fan-out.",
            tuple(p0_vs_bulk),
            1,
            1,
        ),
        Scenario(
            "p0-outside-horizon",
            "A distant P0 must not hijack normal ranking when it cannot become Ready in the horizon.",
            tuple(distant_p0),
            3,
            2,
        ),
        Scenario(
            "graph-compensates-priority",
            "Three P4 unlocks should beat two P1 unlocks outside P0 mode.",
            tuple(count_beats_tier),
            1,
            1,
        ),
        Scenario(
            "priority-without-unlocks",
            "When graph outcomes tie, P1 should beat unspecified and P4.",
            (Issue(1, "p4"), Issue(2), Issue(3, "p1")),
            1,
            3,
        ),
        Scenario(
            "shared-p0-blocker",
            "At equal critical distance, a blocker making two P0s Ready should beat one making one P0 Ready.",
            (
                Issue(1),
                Issue(2),
                Issue(10, "p0", frozenset({1})),
                Issue(11, "p0", frozenset({1})),
                Issue(12, "p0", frozenset({2})),
            ),
            1,
            1,
        ),
        Scenario(
            "minimum-critical-distance",
            "A one-step route to P0 should beat a two-step route to another P0.",
            (
                Issue(1),
                Issue(2),
                Issue(3, blockers=frozenset({2})),
                Issue(10, "p0", frozenset({1})),
                Issue(11, "p0", frozenset({3})),
            ),
            2,
            1,
        ),
        Scenario(
            "downstream-priority-profile",
            "With equal unlock count, outcomes containing P1 should beat exclusively P4 outcomes.",
            (
                Issue(1),
                Issue(2),
                Issue(10, "p4", frozenset({1})),
                Issue(11, "p4", frozenset({1})),
                Issue(20, "p1", frozenset({2})),
                Issue(21, "p4", frozenset({2})),
            ),
            1,
            2,
        ),
        Scenario(
            "earlier-equal-unlock",
            "With equal total and priority, two immediate unlocks should beat a two-step cascade.",
            (
                Issue(1),
                Issue(2),
                Issue(10, blockers=frozenset({1})),
                Issue(11, blockers=frozenset({1})),
                Issue(20, blockers=frozenset({2})),
                Issue(21, blockers=frozenset({20})),
            ),
            2,
            1,
        ),
        Scenario(
            "ready-p0-is-direct",
            "An already Ready P0 should be selected instead of a noncritical fan-out.",
            (
                Issue(1),
                Issue(2, "p0"),
                Issue(10, blockers=frozenset({1})),
                Issue(11, blockers=frozenset({1})),
                Issue(12, blockers=frozenset({1})),
            ),
            1,
            2,
        ),
        Scenario(
            "cycle-not-executable",
            "Members of a dependency cycle must not be selected as Ready work.",
            (
                Issue(1, blockers=frozenset({2})),
                Issue(2, blockers=frozenset({1})),
                Issue(3),
            ),
            2,
            3,
        ),
        Scenario(
            "assigned-candidate",
            "Assigned work is not a candidate, but making assigned downstream work Ready still counts.",
            (
                Issue(1, assigned=True),
                Issue(2),
                Issue(10, "p1", frozenset({2}), assigned=True),
            ),
            1,
            2,
        ),
        Scenario(
            "ready-p0-downstream-p0",
            "Among Ready P0s, enabling another P0 should beat enabling a P4.",
            (
                Issue(1, "p0"),
                Issue(2, "p0"),
                Issue(10, "p0", frozenset({1})),
                Issue(20, "p4", frozenset({2})),
            ),
            1,
            1,
        ),
        Scenario(
            "p0-timing",
            "With equal first P0 progress and total P0s, the earlier P0 cascade should win.",
            (
                Issue(1),
                Issue(2),
                Issue(10, "p0", frozenset({1})),
                Issue(11, blockers=frozenset({1})),
                Issue(12, "p0", frozenset({11})),
                Issue(13, "p0", frozenset({11})),
                Issue(20, "p0", frozenset({2})),
            ),
            3,
            1,
        ),
        Scenario(
            "unknown-external-blocker",
            "A missing blocker is opaque and must keep its dependent non-Ready.",
            (Issue(1, blockers=frozenset({999})), Issue(2)),
            1,
            2,
        ),
        Scenario(
            "priority-conflict-neutral",
            "A Priority conflict remains eligible and compares as neutral rather than P4.",
            (Issue(1, "conflict"), Issue(2, "p4")),
            1,
            1,
        ),
        Scenario(
            "no-candidate",
            "When every open Issue is unavailable or opaque-blocked, there is no recommendation.",
            (Issue(1, assigned=True), Issue(2, blockers=frozenset({999}))),
            1,
            None,
        ),
        Scenario(
            "p0-gate-every-step",
            "A rollout must complete another Ready P0 before following a P4 cascade.",
            (
                Issue(1, "p0"),
                Issue(2, "p0"),
                Issue(10, "p4", frozenset({2})),
                Issue(11, "p4", frozenset({10})),
                *(Issue(number, "p4", frozenset({1})) for number in range(20, 25)),
                *(Issue(number, blockers=frozenset({11})) for number in range(100, 200)),
            ),
            3,
            1,
        ),
    )

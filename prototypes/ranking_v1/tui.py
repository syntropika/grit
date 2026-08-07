#!/usr/bin/env python3
"""PROTOTYPE TUI for inspecting the `next/v1` ranking policy."""

from __future__ import annotations

import argparse

from ranking import rank, scenarios


BOLD = "\033[1m"
DIM = "\033[2m"
RESET = "\033[0m"


def render(index: int, clear: bool = True) -> None:
    cases = scenarios()
    case = cases[index]
    ranked = rank(case)
    winner = ranked[0] if ranked else None
    if clear:
        print("\033[2J\033[H", end="")
    print(f"{BOLD}PROTOTYPE — next/v1 ranking{RESET}")
    print(f"{DIM}Scenario {index + 1}/{len(cases)}{RESET}\n")
    print(f"{BOLD}name:{RESET} {case.name}")
    print(f"{BOLD}question:{RESET} {case.question}")
    print(f"{BOLD}horizon:{RESET} {case.horizon}")
    print(f"{BOLD}expected first:{RESET} {f'#{case.expected}' if case.expected else 'none'}")
    print(f"{BOLD}selected first:{RESET} {f'#{winner.first}' if winner else 'none'}")
    actual = winner.first if winner else None
    print(f"{BOLD}verdict:{RESET} {'EXPECTED' if actual == case.expected else 'SURPRISING'}")
    if winner is None:
        print(f"\n{BOLD}[n]{RESET} next  {BOLD}[p]{RESET} previous  {BOLD}[q]{RESET} quit")
        return
    print(f"{BOLD}mode:{RESET} {winner.mode}")
    print(f"{BOLD}plan:{RESET} {' -> '.join(f'#{n}' for n in winner.sequence)}")
    print(f"{BOLD}critical distance:{RESET} {winner.critical_distance}")
    print(f"{BOLD}P0 curve:{RESET} {winner.p0_curve}")
    print(f"{BOLD}distinct unlocks:{RESET} {sorted(winner.unlocks)}")
    print(f"{BOLD}unlock curve:{RESET} {winner.unlock_curve}")
    print(f"{BOLD}priority profile P1/neutral/P3/P4:{RESET} {winner.priority_profile}")
    print(f"{BOLD}step priorities:{RESET} {winner.step_priorities}")
    print(f"\n{BOLD}[n]{RESET} next  {BOLD}[p]{RESET} previous  {BOLD}[q]{RESET} quit")


def print_all() -> bool:
    all_expected = True
    for index, case in enumerate(scenarios()):
        ranked = rank(case)
        winner = ranked[0] if ranked else None
        actual = winner.first if winner else None
        verdict = "EXPECTED" if actual == case.expected else "SURPRISING"
        all_expected = all_expected and actual == case.expected
        selected = f"#{actual}" if actual is not None else "none"
        plan = winner.sequence if winner else ()
        unlock_count = len(winner.unlocks) if winner else 0
        print(
            f"{index + 1:02d} {case.name:28} {verdict:10} "
            f"first={selected} plan={plan} unlocks={unlock_count}"
        )
    return all_expected


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--all", action="store_true")
    args = parser.parse_args()
    if args.all:
        if not print_all():
            raise SystemExit(1)
        return

    index = 0
    while True:
        render(index)
        command = input("> ").strip().lower()
        if command == "q":
            return
        if command == "n":
            index = (index + 1) % len(scenarios())
        elif command == "p":
            index = (index - 1) % len(scenarios())


if __name__ == "__main__":
    main()

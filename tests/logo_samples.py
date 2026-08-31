"""Print the 5 k9x logo candidates in full color.
Usage: python3 tests/logo_samples.py [A|B|C|D|E]
"""
import sys

C = lambda c: f"\033[{c}m"
RST = "\033[0m"
INFO, OK, WARN, BAD, DIM, TITLE, BOLD = C("38;5;6"), C("38;5;2"), C("38;5;3"), C("38;5;1"), C("38;5;8"), C("38;5;15"), C("1")

S = {}

S["A"] = (
    "A · SOLID SHADOW BLOCKS — classic ANSI-shadow, per-letter theme colors\n\n"
    f"{BOLD}{INFO}██╗  ██╗ {RST}{BOLD}{OK} █████╗ {RST}{BOLD}{WARN}██╗  ██╗{RST}\n"
    f"{INFO}██║ ██╔╝{RST}{OK}██╔══██╗{RST}{WARN}╚██╗██╔╝{RST}\n"
    f"{INFO}█████╔╝ {RST}{OK}███████║{RST}{WARN} ╚███╔╝ {RST}\n"
    f"{INFO}██╔═██╗ {RST}{OK}██╔══██║{RST}{WARN} ██╔██╗ {RST}\n"
    f"{DIM}██║  ██╗{RST}{OK}██║  ██║{RST}{WARN}██╔╝ ██╗{RST}\n"
    f"{DIM}╚═╝  ╚═╝╚═╝  ╚═╝╚═╝  ╚═╝{RST}\n"
    f"{DIM}       event-driven kubernetes TUI{RST}\n"
)

S["B"] = (
    "B · GRADIENT SHADE — soft pixel fade, red→green→yellow journey\n\n"
    f"{BAD}▓▓░  ▓▓▓ {RST}{OK}░▓▓▓▓▓ {RST}{WARN}▓▓░  ▓▓{RST}\n"
    f"{BAD}▓▓▓ ▓▓░ {RST}{OK}▓▓░░░▓▓{RST}{WARN}░▓▓ ▓▓░{RST}\n"
    f"{BAD}▓▓▓▓▓▓░ {RST}{OK}▓▓░░░▓▓{RST}{WARN} ░▓▓▓░ {RST}\n"
    f"{BAD}▓▓▓ ▓▓░ {RST}{OK}▓▓░░░▓▓{RST}{WARN}░▓▓ ▓▓░{RST}\n"
    f"{BAD}▓▓░  ▓▓▓ {RST}{OK}░▓▓▓▓▓ {RST}{WARN}▓▓░  ▓▓{RST}\n"
    f"{DIM}       event-driven kubernetes TUI{RST}\n"
)

S["C"] = (
    "C · FRAMED BADGE — thin-stroke letters in a double frame\n\n"
    f"{DIM}╔══════════════════════╗{RST}\n"
    f"{DIM}║{RST} {INFO}╷   ┌─┐{RST}   {WARN}╲ ╱{RST}   {DIM}║{RST}\n"
    f"{DIM}║{RST} {INFO}├─┤ │ │{RST}   {WARN} ╳ {RST}   {DIM}║{RST}      {TITLE}{BOLD}k9x{RST}\n"
    f"{DIM}║{RST} {INFO}│ │ │ │{RST}   {WARN}╱ ╲{RST}   {DIM}║{RST}\n"
    f"{DIM}║{RST} {INFO}╵ ╵ ╰─╯{RST}        {DIM}║{RST}\n"
    f"{DIM}╚══════════════════════╝{RST}\n"
)

S["D"] = (
    "D · ORBIT MODULES — three pods on a rail, mission-control vibe\n\n"
    f"{DIM}──────────────────────────────{RST}\n"
    f"  {INFO}╭───╮{RST}  {OK}╭───╮{RST}  {WARN}╭───╮{RST}\n"
    f"  {INFO}│ k │{RST}  {OK}│ 9 │{RST}  {WARN}│ x │{RST}\n"
    f"  {INFO}╰───╯{RST}  {OK}╰───╯{RST}  {WARN}╰───╯{RST}\n"
    f"{DIM}──────────────────────────────{RST}\n"
    f"{TITLE}  ●── event-driven kubernetes TUI ──●{RST}\n"
)

S["E"] = (
    "E · CIRCUIT SPEED — double-line chips with speed rails\n\n"
    f"{DIM}─────────────────────────────{RST}\n"
    f"  {BOLD}{INFO}╔══╗{RST}{BOLD}{OK}╔══╗{RST}{BOLD}{WARN}╔═╗{RST}   {DIM}──→{RST}\n"
    f"  {BOLD}{INFO}║ k{RST}{BOLD}{OK}║ 9{RST}{BOLD}{WARN}║ x{RST}{RST}  {DIM}──→{RST}   {TITLE}{BOLD}fast kubernetes TUI{RST}\n"
    f"  {BOLD}{INFO}╚══╝{RST}{BOLD}{OK}╚══╝{RST}{BOLD}{WARN}╚═╝{RST}   {DIM}──→{RST}\n"
    f"{DIM}─────────────────────────────{RST}\n"
)

pick = sys.argv[1].upper() if len(sys.argv) > 1 else None
for key in (["A", "B", "C", "D", "E"] if pick not in S else [pick]):
    print(S[key])

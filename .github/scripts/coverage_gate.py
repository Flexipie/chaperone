"""Fails when a branch side or a region in the given files ran in no test.

Reads the full `cargo llvm-cov --json` export. llvm's file summary scores a
function by its best single build, and the lib unit tests and the integration
tests are separate builds, so this merges the builds instead. A region that is
only a `?` is exempt. Most propagate a SQLite error no test reaches without
fault injection. The rest propagate a refusal whose own check is gated: the
schema re-check, a foreign log's unknown kind or bad payload, and a fold that
changes no row.
"""

import json
import sys
from collections import defaultdict


def missed(data, path):
    """The branches with a side never taken and the regions never run in
    `path`, as (kind, line, col, end_line, end_col), a `?`-only region exempt."""
    src = open(path).read().split("\n")
    # Keyed by function, span and position among same-span records, so the
    # copies rustc makes of a guard on an or-pattern stay apart.
    taken = defaultdict(lambda: [0, 0])
    for fn in data["functions"]:
        if not fn["filenames"][0].endswith("/" + path):
            continue
        start = tuple(fn["regions"][0][:2])
        seen = defaultdict(int)
        records = [("branch", b, b[5]) for b in fn["branches"]]
        records += [("region", r, 1) for r in fn["regions"] if r[7] == 0]
        for kind, r, other in records:
            span = tuple(r[:4])
            key = (kind, start, span, seen[(kind, span)])
            seen[(kind, span)] += 1
            taken[key][0] += r[4]
            taken[key][1] += other
    out = []
    for (kind, _, (line, col, end_line, end_col), _), (hits, other) in sorted(taken.items()):
        if kind == "branch" and (hits == 0 or other == 0):
            out.append((kind, line, col, end_line, end_col))
        elif kind == "region" and hits == 0:
            text = src[line - 1][col - 1 : end_col - 1] if line == end_line else ""
            if text != "?":
                out.append((kind, line, col, end_line, end_col))
    return out


if __name__ == "__main__":
    data = json.load(open(sys.argv[1]))["data"][0]
    failed = False
    for path in sys.argv[2:]:
        file = next((f for f in data["files"] if f["filename"].endswith("/" + path)), None)
        if file is None or not file["branches"]:
            print(f"{path}: no branch data")
            failed = True
            continue
        found = []
        funcs = file["summary"]["functions"]
        if funcs["covered"] != funcs["count"]:
            found.append(f"{funcs['count'] - funcs['covered']} functions never called")
        for kind, line, col, end_line, end_col in missed(data, path):
            if kind == "branch":
                found.append(f"branch {line}:{col}")
            else:
                found.append(f"region {line}:{col}-{end_line}:{end_col}")
        if found:
            print(f"{path}: {found}")
            failed = True
    sys.exit(1 if failed else 0)

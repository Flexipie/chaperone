"""Fails when a branch side or a region in the given files ran in no test.

Reads the full `cargo llvm-cov --json` export. llvm's file summary scores a
function by its best single build, and the lib unit tests and the integration
tests are separate builds, so this merges the builds that ran instead. A
region that is only the `?` of a fallible call is exempt: no test reaches the
error arm of a SQLite call without fault injection.
"""

import json
import sys
from collections import defaultdict

data = json.load(open(sys.argv[1]))["data"][0]
failed = False
for path in sys.argv[2:]:
    src = open(path).read().split("\n")
    file = next((f for f in data["files"] if f["filename"].endswith("/" + path)), None)
    if file is None or not file["branches"]:
        print(f"{path}: no branch data")
        failed = True
        continue
    missed = []
    funcs = file["summary"]["functions"]
    if funcs["covered"] != funcs["count"]:
        missed.append(f"{funcs['count'] - funcs['covered']} functions never called")
    # Keyed by function, span and position among same-span records, so the
    # copies rustc makes of a guard on an or-pattern stay apart. A copy that
    # never ran keeps regions the running copies optimised away; skip it.
    taken = defaultdict(lambda: [0, 0])
    for fn in data["functions"]:
        if fn["count"] == 0 or not fn["filenames"][0].endswith("/" + path):
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
    for (kind, _, (line, col, end_line, end_col), _), (hits, other) in sorted(taken.items()):
        if kind == "branch" and (hits == 0 or other == 0):
            missed.append(f"branch {line}:{col}")
        elif kind == "region" and hits == 0:
            text = src[line - 1][col - 1 : end_col - 1] if line == end_line else ""
            if text != "?":
                missed.append(f"region {line}:{col}-{end_line}:{end_col}")
    if missed:
        print(f"{path}: {missed}")
        failed = True
sys.exit(1 if failed else 0)

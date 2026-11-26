"""Prints the lines added under crates/*/src since the given base that no test
ran, per file, then a total. Report-only: always exits 0.

Reads the same `cargo llvm-cov --json` export and missed regions as
coverage_gate.py, `?` exempt. A line a missed region only starts or ends on,
with code of its own beside it, partly ran and is left out, and so is a lone
`}` region: llvm's implicit else of an `if` whose body always ran.
"""

import json
import re
import subprocess
import sys
from collections import defaultdict

from coverage_gate import missed

data = json.load(open(sys.argv[1]))["data"][0]
diff = subprocess.run(
    ["git", "diff", "--unified=0", sys.argv[2], "--", "crates/*/src/*.rs"],
    capture_output=True, text=True, check=True,
).stdout
added = defaultdict(set)
path = None
for row in diff.splitlines():
    if row.startswith("+++ "):
        path = row[6:] if row.startswith("+++ b/") else None
    elif row.startswith("@@") and path:
        start, count = re.search(r"\+(\d+)(?:,(\d+))?", row).groups()
        added[path].update(range(int(start), int(start) + int(count or 1)))

files = total = 0
for path in sorted(added):
    src = open(path).read().split("\n")
    if not any(f["filename"].endswith("/" + path) for f in data["files"]):
        if any(re.search(r"\bfn\b", src[n - 1]) for n in added[path]):
            print(f"{path}: no coverage data")
        continue
    lines = set()
    for kind, line, col, end_line, end_col in missed(data, path):
        if kind != "region" or line == end_line and not src[line - 1][col - 1 : end_col - 1].strip(" },;"):
            continue
        first = line if not src[line - 1][: col - 1].strip() else line + 1
        last = end_line if not src[end_line - 1][end_col - 1 :].strip(" },;") else end_line - 1
        lines.update(range(first, last + 1))
    lines = sorted(lines & added[path])
    if not lines:
        continue
    ranges, lo = [], lines[0]
    for a, b in zip(lines, lines[1:] + [None]):
        if b != a + 1:
            ranges.append(f"{lo}" if lo == a else f"{lo}-{a}")
            lo = b
    print(f"{path}: {', '.join(ranges)}")
    files += 1
    total += len(lines)
print(f"total: {total} lines in {files} files")

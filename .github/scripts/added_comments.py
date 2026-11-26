"""Report-only (#320): each `//` line a PR adds under crates/*/src with no
issue, URL or doc link, and each `///` it adds on an item that is not `pub`.
Usage: added_comments.py [base], default HEAD^1 (CI's merge commit)."""

import re
import subprocess
import sys

LINK = re.compile(r"#\d+|https?://|\.md\b|ADR \d+")
ITEM = re.compile(
    r"(pub\([^)]*\)\s+)?((async|const|unsafe|extern(\s+\"[^\"]*\")?)\s+)*"
    r"(fn|struct|enum|union|trait|type|mod|const|static|impl|macro_rules!)\b"
)


def git(*args):
    return subprocess.run(["git", *args], capture_output=True, text=True, check=True).stdout


def item_after(lines, i):
    """The line the doc comment at index i documents: past the doc block and attributes."""
    depth = 0
    while i < len(lines):
        t = lines[i].strip()
        if depth == 0 and not t.startswith(("///", "#[")):
            return t
        if depth or t.startswith("#["):
            depth += t.count("[") - t.count("]")
        i += 1
    return ""


base = sys.argv[1] if len(sys.argv) > 1 else "HEAD^1"
diff = git("diff", "-U0", "--no-color", base, "HEAD", "--", ":(glob)crates/*/src/**/*.rs")
found, path, new, files, seen = 0, None, 0, {}, set()
for line in diff.splitlines():
    if line.startswith("+++ "):
        path = line[6:] if line.startswith("+++ b/") else None
    elif line.startswith("@@"):
        new = int(re.match(r"@@ -\S+ \+(\d+)", line).group(1))
    elif line.startswith("+") and path:
        t = line[1:].strip()
        msg = None
        if t.startswith("///"):
            lines = files.setdefault(path, git("show", f"HEAD:{path}").splitlines())
            item = item_after(lines, new - 1)
            key = (path, item)
            if ITEM.match(item) and key not in seen:
                seen.add(key)
                msg = f"doc comment on an item that is not pub: {item}"
        elif t.startswith("//") and not t.startswith("//!") and not LINK.search(t):
            msg = f"comment with no link: {t}"
        if msg:
            found += 1
            print(f"::warning file={path},line={new}::{msg}")
        new += 1
print(f"added comments to look at: {found}")

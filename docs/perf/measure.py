#!/usr/bin/env python3
"""Measures a chaperone release binary the R14 way, for docs/perf/v0-review.md.

    python3 -I docs/perf/measure.py <before-binary> <after-binary> [--items 3000] [--open-specs 5]

The first binary seeds the shared snapshot, so list the oldest schema first:
a newer binary migrates its copy, an older one refuses a newer file.

Everything runs in a fresh temp dir: every call passes --db there, and HOME
and XDG_DATA_HOME point there too, so a missed --db can never reach a real
ledger. Before a temp dir is removed, the run fails if the default ledger
path appeared inside it, which is where a missed --db lands; writes by other
processes to the real ledger don't count. A binary that built its path by hand
instead of from HOME would get past this. Startup method from R14: 8
unmeasured spawns, then 100 samples, wall time around the spawn. Prints
markdown tables. Python stdlib only.
"""

import argparse
import json
import os
import re
import shutil
import sqlite3
import subprocess
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor

WARM, SAMPLES = 8, 100
RSS_RUNS = 3
HEAD = "abcdef1"


class Bench:
    def __init__(self, binary, root):
        self.bin = os.path.abspath(binary)
        self.db = os.path.join(root, "ledger.db")
        self.env = {
            "PATH": "/usr/bin:/bin",
            "HOME": root,
            "XDG_DATA_HOME": root,
            "DAISY_AGENT": "bench",
            "DAISY_SESSION": "s1",
        }

    def argv(self, args):
        return [self.bin, "--db", self.db, *args]

    def env_for(self, session):
        return {**self.env, "DAISY_SESSION": session} if session else self.env

    def run(self, *args, stdin=None, session=None):
        out = subprocess.run(
            self.argv(args), input=stdin, capture_output=True, text=True,
            env=self.env_for(session),
        )
        if out.returncode != 0:
            raise SystemExit(f"{args} exited {out.returncode}: {out.stderr[:400]}")
        return json.loads(out.stdout)["data"] if out.stdout.startswith("{") else out.stdout

    def has(self, command):
        """Whether this binary has the command (older ones lack newer ones)."""
        return subprocess.run(self.argv([command, "--help"]), capture_output=True,
                              env=self.env).returncode == 0

    def timed(self, make, session=None, warm=WARM, samples=SAMPLES):
        """make(i) -> (args, stdin). Wall ms of each sample after the warmup."""
        times = []
        for i in range(warm + samples):
            args, stdin = make(i)
            t0 = time.perf_counter_ns()
            out = subprocess.run(
                self.argv(args), input=stdin, stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE, text=True, env=self.env_for(session),
            )
            dt = (time.perf_counter_ns() - t0) / 1e6
            if out.returncode != 0:
                raise SystemExit(f"{args} exited {out.returncode}: {out.stderr[:400]}")
            if i >= warm:
                times.append(dt)
        return times

    def create(self, n, **kw):
        ids = []
        for _ in range(n):
            args = ["work", "create", "--title", kw.get("title", "bench item")]
            for flag in ("source", "source_id", "spec", "lane"):
                if flag in kw:
                    args += [f"--{flag.replace('_', '-')}", kw[flag]]
            ids.append(self.run(*args)["id"])
        return ids


def pct(times, p):
    s = sorted(times)
    return s[min(len(s) - 1, int(round(p / 100 * (len(s) - 1))))]


def row(name, times):
    return f"| {name} | {pct(times, 50):.2f} | {pct(times, 99):.2f} | {max(times):.2f} |"


def handoff(status="active"):
    return json.dumps({
        "summary": "bench handoff " + "x" * 200,
        "status": status,
        "changed_files": ["crates/a.rs", "docs/b.md"],
        "verification": [{"command": "cargo nextest run", "status": "passed"}],
    })


def spec_body(parent, keys):
    slices = "\n".join(f"## slice {i + 1}: slice {i + 1}  ({k})\n\nDone when it works.\n"
                       for i, k in enumerate(keys))
    return f"# {parent}\n\n{slices}\n## decisions\n\n- [slice 1] Q: one? A: yes.\n"


def seed(b, items, open_specs):
    """Realistic ledger through the CLI: specs with slices, and per item a
    create, claim, heartbeat, two handoffs and a check from a fresh session.
    Only the newest open_specs specs (10 items each) stay open; older items
    are cancelled, as finished work leaves the board."""
    t0 = time.monotonic()
    per_spec = 10
    specs = items // per_spec
    for s in range(specs):
        parent = f"bench/repo#{100000 + s}"
        keys = [f"bench/repo#{200000 + s * per_spec + i}" for i in range(per_spec)]
        b.run("spec", "create", "--parent", parent, "--file", "-",
              stdin=spec_body(parent, keys))
        for i, key in enumerate(keys):
            [lane] = b.create(1, source="github", source_id=key, spec=parent,
                              lane=f"{parent}:{chr(65 + i)}")
            b.run("work", "claim", str(lane))
            b.run("work", "heartbeat", str(lane))
            b.run("work", "handoff", str(lane), stdin=handoff())
            b.run("work", "handoff", str(lane), stdin=handoff("blocked"))
            b.run("check", "record", "--work", str(lane), "--check", "smoke",
                  "--head", HEAD, "--outcome", "ran", session="checker")
            if s < specs - open_specs:
                b.run("work", "cancel", str(lane))
    con = sqlite3.connect(f"file:{b.db}?mode=ro", uri=True)
    counts = {t: con.execute(f"SELECT count(*) FROM {t}").fetchone()[0]
              for t in ("events", "lanes", "specs", "gate_results")}
    counts["open lanes"] = con.execute(
        "SELECT count(*) FROM lanes WHERE status NOT IN ('done', 'cancelled')").fetchone()[0]
    con.close()
    return counts, time.monotonic() - t0


def hot(b, label, spec_key):
    """The hot commands, each on items prepared outside the timed loop, then
    the max RSS of each on items the loop did not touch."""
    n = WARM + SAMPLES
    fresh = b.create(n + RSS_RUNS)
    held = b.create(n + RSS_RUNS)
    for lane in held:
        b.run("work", "claim", str(lane))
    check = ["check", "record", "--check", "smoke", "--head", HEAD, "--outcome", "ran"]
    cmds = [
        ("work get", lambda i: (["work", "get", str(held[0])], None), None),
        ("work list", lambda i: (["work", "list"], None), None),
        ("work claim", lambda i: (["work", "claim", str(fresh[i])], None), None),
        ("work heartbeat", lambda i: (["work", "heartbeat", str(held[0])], None), None),
        ("work handoff", lambda i: (["work", "handoff", str(held[i])], handoff()), None),
        ("spec get --key", lambda i: (["spec", "get", "--key", spec_key], None), None),
        ("check record", lambda i: ([*check, "--work", str(held[i])], None), "checker"),
    ]
    if b.has("dossier"):
        # The key's slice, lane and handoffs come from the seed; 20 notes on top.
        for n in range(20):
            b.run("dossier", "note", spec_key, "--note", f"bench note {n} " + "x" * 500,
                  "--link", "adr:docs/decisions/0007-issue-dossier.md")
        cmds += [
            ("dossier get", lambda i: (["dossier", "get", spec_key], None), None),
            ("dossier note",
             lambda i: (["dossier", "note", spec_key, "--note", f"timed note {i}"], None), None),
        ]
    rows = [row(name, b.timed(make, session=session)) for name, make, session in cmds]
    print(f"\n### Hot commands, {label} ledger (ms)\n")
    print("| Command | p50 | p99 | max |\n| --- | --- | --- | --- |")
    print("\n".join(rows))
    print(f"\nM1 max RSS, {label} ledger (MiB, {RSS_RUNS} runs): " + ", ".join(
        f"{name} {rss_range(b, make, session)}" for name, make, session in cmds))


def max_rss(b, *args, stdin=None, session=None):
    out = subprocess.run(["/usr/bin/time", "-l", *b.argv(args)], input=stdin,
                         capture_output=True, text=True, env=b.env_for(session))
    if out.returncode != 0:
        raise SystemExit(f"{args} exited {out.returncode}: {out.stderr[:400]}")
    m = re.search(r"(\d+)\s+maximum resident set size", out.stderr)
    return int(m.group(1)) / 2**20


def rss_range(b, make, session=None):
    """make(i) -> (args, stdin), called with the indexes after the timed loop's."""
    mibs = []
    for i in range(WARM + SAMPLES, WARM + SAMPLES + RSS_RUNS):
        args, stdin = make(i)
        mibs.append(max_rss(b, *args, stdin=stdin, session=session))
    return f"{min(mibs):.2f} to {max(mibs):.2f}"


def contention(b, writers=50, each=20):
    """`writers` processes at once, each `each` creates in a row, against the
    same count run one at a time. The difference is lock wait."""
    def one(_):
        times = []
        for _ in range(each):
            t0 = time.perf_counter_ns()
            b.run("work", "create", "--title", "contended")
            times.append((time.perf_counter_ns() - t0) / 1e6)
        return times

    t0 = time.monotonic()
    serial = one(0)
    serial_s = time.monotonic() - t0
    t0 = time.monotonic()
    with ThreadPoolExecutor(writers) as pool:
        times = [t for ts in pool.map(one, range(writers)) for t in ts]
    wall = time.monotonic() - t0
    print(f"\n### Concurrent CLI writers ({writers} x {each} `work create`)\n")
    print(f"- one writer, {each} creates: {serial_s:.2f} s, p50 {pct(serial, 50):.2f} ms")
    print(f"- {writers} writers, {writers * each} creates: {wall:.2f} s wall, "
          f"{writers * each / wall:.0f} commits/s, per call p50 {pct(times, 50):.2f} ms, "
          f"p99 {pct(times, 99):.2f} ms, max {max(times):.2f} ms")


PLANS = {
    "work get": "SELECT (SELECT idempotency_key FROM events WHERE lane_id = lanes.id "
                "ORDER BY id LIMIT 1) FROM lanes WHERE id = ?1",
    "work list": "SELECT id FROM lanes WHERE status NOT IN ('done', 'cancelled') "
                 "ORDER BY updated_at DESC, id DESC LIMIT 50",
    "spec by key (NOCASE)": "SELECT id FROM specs WHERE external_key = ?1 COLLATE NOCASE",
    "spec by slice key (spec_keys, v3)":
        "SELECT s.id FROM spec_keys k JOIN specs s ON s.id = k.spec_id WHERE k.key = ?1 "
        "ORDER BY s.updated_at DESC, s.id DESC LIMIT 1",
    "slice lane, linked to the spec":
        "SELECT id, status, pr_ref FROM lanes WHERE spec_id = ?2 AND source IN ('linear', "
        "'issue', 'github') AND source_id = ?1 COLLATE NOCASE ORDER BY id DESC LIMIT 1",
    "slice lane, fallback when none is linked":
        "SELECT id, status, pr_ref FROM lanes WHERE source IN ('linear', 'issue', 'github') "
        "AND source_id = ?1 COLLATE NOCASE ORDER BY id DESC LIMIT 1",
    "slice lane by source_id (NOCASE), before":
        "SELECT id, status, pr_ref FROM lanes WHERE source IN ('linear', 'issue', 'github') "
        "AND source_id = ?1 COLLATE NOCASE ORDER BY (spec_id IS ?2) DESC, id DESC LIMIT 1",
    "has_held": "SELECT 1 FROM events WHERE lane_id = ?1 AND kind IN "
                "('lane.claimed', 'lane.taken_over') AND actor_session = ?2",
    "idempotency by_key": "SELECT id FROM events WHERE idempotency_key = ?1",
    "jobs (JOB_EVENTS)": "SELECT id FROM events WHERE kind IN ('check.requested', "
                         "'check.finished') AND json_extract(payload_json, '$.job') = ?1 "
                         "ORDER BY kind = 'check.finished' DESC, id DESC LIMIT 1",
    "latest handoff": "SELECT id FROM events WHERE lane_id = ?1 AND kind IN "
                      "('lane.handed_off', 'phase.entered') ORDER BY id DESC LIMIT 1",
    "dossier notes (events_dossier, v4)":
        "SELECT id FROM events WHERE kind = 'dossier.noted' "
        "AND json_extract(payload_json, '$.key') = ?1",
    "next_id": "SELECT coalesce(max(id), 0) + 1 FROM lanes",
}


def plans(b):
    con = sqlite3.connect(f"file:{b.db}?mode=ro", uri=True)
    print("\n### Query plans (realistic ledger)\n")
    print("| Read | Plan |\n| --- | --- |")
    for name, sql in PLANS.items():
        n = sql.count("?")
        try:
            detail = "; ".join(r[3] for r in con.execute(f"EXPLAIN QUERY PLAN {sql}", [None] * n))
        except sqlite3.OperationalError as e:  # a table an older schema lacks
            detail = f"n/a ({e})"
        print(f"| {name} | {detail} |")
    # A missing job: a scan reads every event either way, an index search none.
    jobs = PLANS["jobs (JOB_EVENTS)"]
    times = []
    for i in range(WARM + SAMPLES):
        t0 = time.perf_counter_ns()
        con.execute(jobs, [10**9]).fetchall()
        if i >= WARM:
            times.append((time.perf_counter_ns() - t0) / 1e6)
    print(f"\njob lookup in the process (ms): p50 {pct(times, 50):.3f}, "
          f"p99 {pct(times, 99):.3f}, max {max(times):.3f}")
    con.close()


DEEP_LANES, DEEP_EVENTS = 3, 300


def deepen(b):
    """A few long-lived lanes: the handoff gate projects every event of the
    lane inside the write, so its cost grows with events per lane."""
    deep = b.create(DEEP_LANES, title="deep lane")
    for lane in deep:
        b.run("work", "claim", str(lane))
        # A new head each time: the same check on the same head is replayed,
        # not written.
        for n in range(DEEP_EVENTS):
            b.run("check", "record", "--work", str(lane), "--check", "smoke",
                  "--head", f"{n:07x}", "--outcome", "ran", session="checker")
    return deep


def measure(binary, snapshot, items, deep):
    root = tempfile.mkdtemp(prefix="chaperone-perf-")
    try:
        b = Bench(binary, root)
        print(f"\n## {b.bin}\n\nbinary {os.path.getsize(b.bin) / 2**20:.2f} MiB")
        print("\n### Startup (ms)\n\n| Command | p50 | p99 | max |\n| --- | --- | --- | --- |")
        spawn = []
        for i in range(WARM + SAMPLES):
            t0 = time.perf_counter_ns()
            subprocess.run(["/usr/bin/true"])
            if i >= WARM:
                spawn.append((time.perf_counter_ns() - t0) / 1e6)
        print(row("/usr/bin/true (harness floor)", spawn))
        print(row("S1 --version", b.timed(lambda i: (["--version"], None))))
        # Empty: a fresh ledger holding only what each command needs.
        b.run("migrate")
        print(row("S2+ work list, empty ledger", b.timed(lambda i: (["work", "list"], None))))
        print(f"\nM1 max RSS (MiB, {RSS_RUNS} runs): "
              f"--version {rss_range(b, lambda i: (['--version'], None))}, "
              f"work list (empty) {rss_range(b, lambda i: (['work', 'list'], None))}")
        parent, key = "bench/repo#1", "bench/repo#2"
        b.run("spec", "create", "--parent", parent, "--file", "-", stdin=spec_body(parent, [key]))
        hot(b, "empty", key)

        for suffix in ("", "-wal", "-shm"):
            if os.path.exists(b.db + suffix):
                os.remove(b.db + suffix)
            if os.path.exists(snapshot + suffix):
                shutil.copy(snapshot + suffix, b.db + suffix)
        # The snapshot is at the first binary's version; a newer one upgrades it.
        t0 = time.perf_counter_ns()
        b.run("migrate")
        print(f"\nmigrate on the snapshot: {(time.perf_counter_ns() - t0) / 1e6:.0f} ms")
        hot(b, f"realistic ({items} items)", f"bench/repo#{200000 + items // 2}")
        deep_handoff = lambda i: (["work", "handoff", str(deep[i % len(deep)])], handoff())
        print(row(f"work handoff, lane with {DEEP_EVENTS}+ events", b.timed(deep_handoff)))
        print(f"\nM1 max RSS, realistic (MiB, {RSS_RUNS} runs): work board "
              f"{rss_range(b, lambda i: (['work', 'board'], None))}, "
              f"work handoff, deep lane {rss_range(b, deep_handoff)}")
        board = b.timed(lambda i: (["work", "board"], None), warm=2, samples=20)
        print(f"\nwork board, realistic: p50 {pct(board, 50):.2f} ms, "
              f"max {max(board):.2f} ms (20 samples)")
        plans(b)
        contention(b)
    finally:
        check_sandbox(root)
        shutil.rmtree(root)


LIVE_LANES, REPO_FILES = 50, 400


def git(cwd, *args):
    subprocess.run(["git", "-c", "user.email=b@b", "-c", "user.name=b", *args], cwd=cwd,
                   check=True, capture_output=True, env={"PATH": "/usr/bin:/bin", "HOME": cwd})


def live_lanes(binary):
    """`overlap` at 50 open lanes, each a worktree of one repo with a commit,
    an uncommitted edit and a new file, and a handoff listing 10 paths; 5
    paths each are shared with the next lane. Its own ledger and repo, so
    the realistic snapshot stays as it was."""
    root = tempfile.mkdtemp(prefix="chaperone-lanes-")
    try:
        b = Bench(binary, root)
        has = "overlap" in b.run("--help")
        b.run("migrate")
        git(root, "init", "-q", "--bare", "-b", "main", "origin.git")
        git(root, "clone", "-q", "origin.git", "main")
        repo = os.path.join(root, "main")
        names = [f"src/m{i // 20}/f{i}.rs" for i in range(REPO_FILES)]
        for n in names:
            os.makedirs(os.path.dirname(os.path.join(repo, n)), exist_ok=True)
            with open(os.path.join(repo, n), "w") as f:
                f.write(f"// {n}\n")
        git(repo, "add", "-A")
        git(repo, "commit", "-qm", "init")
        git(repo, "push", "-q", "origin", "main")
        git(repo, "remote", "set-head", "origin", "main")
        for i in range(LIVE_LANES):
            wt = os.path.join(root, f"wt{i}")
            git(repo, "worktree", "add", "-q", wt, "-b", f"lane{i}")
            mine = [names[(i * 5 + k) % REPO_FILES] for k in range(10)]
            for n in mine[:8]:
                with open(os.path.join(wt, n), "a") as f:
                    f.write("x\n")
            git(wt, "commit", "-qam", "lane")
            for n in mine[8:]:
                with open(os.path.join(wt, n), "a") as f:
                    f.write("y\n")
            with open(os.path.join(wt, f"src/new{i}.rs"), "w") as f:
                f.write("new\n")
            lane = b.run("work", "create", "--title", f"lane {i}", "--repo", "bench/repo")["id"]
            b.run("work", "claim", str(lane), "--worktree", wt)
            b.run("work", "handoff", str(lane), "--stdin", stdin=json.dumps({
                "summary": "bench", "status": "active", "changed_files": mine}))
        board = b.timed(lambda i: (["work", "board"], None), warm=2, samples=20)
        ready = b.timed(lambda i: (["work", "ready", "1", "--clear"], None), session="owner",
                        warm=2, samples=20)
        print(f"\n### {LIVE_LANES} open lanes with worktrees (ms)\n")
        print("| Command | p50 | p99 | max |\n| --- | --- | --- | --- |")
        if has:
            print(row("overlap (commits in 50 worktrees)",
                      b.timed(lambda i: (["overlap"], None), warm=2, samples=20)))
            print(row("overlap --uncommitted (also stats 50 worktrees)",
                      b.timed(lambda i: (["overlap", "--uncommitted"], None), warm=2, samples=20)))
        print(row("work board", board))
        print(row("work ready (one board item)", ready))
        print(f"\nM1 max RSS: work board {max_rss(b, 'work', 'board'):.2f} MiB")
        if has:
            shared = sum(f["shared"] for f in b.run("overlap", "--uncommitted")["files"])
            print(f"overlap {max_rss(b, 'overlap'):.2f} MiB, --uncommitted "
                  f"{max_rss(b, 'overlap', '--uncommitted'):.2f} MiB (largest of it and its git "
                  f"children), {shared} shared files")
    finally:
        check_sandbox(root)
        shutil.rmtree(root)


def check_sandbox(root):
    """A missed --db resolves the default ledger from HOME, so into root."""
    stray = os.path.join(root, "Library", "Application Support", "chaperone")
    assert not os.path.exists(stray), f"a call missed --db: {stray}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("binaries", nargs="+", help="oldest first; the first one seeds the shared snapshot")
    ap.add_argument("--items", type=int, default=3000)
    ap.add_argument("--open-specs", type=int, default=5,
                    help="specs left open, 10 items each (100 with --items 1000 for 1,000 open)")
    a = ap.parse_args()
    print(f"python sqlite {sqlite3.sqlite_version} (query plans and the in-process job lookup)")
    seed_root = tempfile.mkdtemp(prefix="chaperone-seed-")
    try:
        # One seeded snapshot, checkpointed, so every binary reads the same data.
        s = Bench(a.binaries[0], seed_root)
        s.run("migrate")
        counts, secs = seed(s, a.items, a.open_specs)
        deep = deepen(s)
        con = sqlite3.connect(s.db)
        con.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        counts["events with deep lanes"] = con.execute("SELECT count(*) FROM events").fetchone()[0]
        con.close()
        print(f"\nSeeded {counts} ({DEEP_LANES} deep lanes x {DEEP_EVENTS} checks) "
              f"through the CLI in {secs:.0f} s, ledger {os.path.getsize(s.db) / 2**20:.1f} MiB")
        for binary in a.binaries:
            measure(binary, s.db, a.items, deep)
            live_lanes(binary)
    finally:
        check_sandbox(seed_root)
        shutil.rmtree(seed_root)


if __name__ == "__main__":
    main()

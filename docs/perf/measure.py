#!/usr/bin/env python3
"""Measures a seamlane release binary the R14 way, for docs/perf/v0-review.md.

    python3 -I docs/perf/measure.py <before-binary> <after-binary> [--items 3000]

The first binary seeds the shared snapshot.

Everything runs in a fresh temp dir: every call passes --db there, and HOME
and XDG_DATA_HOME point there too, so a missed --db can never reach a real
ledger. Startup method from R14: 8 unmeasured spawns, then 100 samples, wall
time around the spawn. Prints markdown tables. Python stdlib only.
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


OPEN_SPECS = 5


def seed(b, items):
    """Realistic ledger through the CLI: specs with slices, and per item a
    create, claim, heartbeat, two handoffs and a check from a fresh session.
    Only the newest OPEN_SPECS specs stay open; older items are cancelled, as
    finished work leaves the board."""
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
            if s < specs - OPEN_SPECS:
                b.run("work", "cancel", str(lane))
    con = sqlite3.connect(f"file:{b.db}?mode=ro", uri=True)
    counts = {t: con.execute(f"SELECT count(*) FROM {t}").fetchone()[0]
              for t in ("events", "lanes", "specs", "gate_results")}
    counts["open lanes"] = con.execute(
        "SELECT count(*) FROM lanes WHERE status NOT IN ('done', 'cancelled')").fetchone()[0]
    con.close()
    return counts, time.monotonic() - t0


def hot(b, label, spec_key):
    """The hot commands, each on items prepared outside the timed loop."""
    n = WARM + SAMPLES
    fresh = b.create(n)
    held = b.create(n)
    for lane in held:
        b.run("work", "claim", str(lane))
    rows = [
        row("work get", b.timed(lambda i: (["work", "get", str(held[0])], None))),
        row("work list", b.timed(lambda i: (["work", "list"], None))),
        row("work claim", b.timed(lambda i: (["work", "claim", str(fresh[i])], None))),
        row("work heartbeat", b.timed(lambda i: (["work", "heartbeat", str(held[0])], None))),
        row("work handoff", b.timed(lambda i: (["work", "handoff", str(held[i])], handoff()))),
        row("spec get --key", b.timed(lambda i: (["spec", "get", "--key", spec_key], None))),
        row("check record", b.timed(
            lambda i: (["check", "record", "--work", str(held[i]), "--check", "smoke",
                        "--head", HEAD, "--outcome", "ran"], None), session="checker")),
    ]
    print(f"\n### Hot commands, {label} ledger (ms)\n")
    print("| Command | p50 | p99 | max |\n| --- | --- | --- | --- |")
    print("\n".join(rows))


def max_rss(b, *args):
    out = subprocess.run(["/usr/bin/time", "-l", *b.argv(args)], capture_output=True,
                         text=True, env=b.env)
    if out.returncode != 0:
        raise SystemExit(f"{args} exited {out.returncode}: {out.stderr[:400]}")
    m = re.search(r"(\d+)\s+maximum resident set size", out.stderr)
    return int(m.group(1)) / 2**20


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
    "next_id": "SELECT coalesce(max(id), 0) + 1 FROM lanes",
}


def plans(b):
    con = sqlite3.connect(f"file:{b.db}?mode=ro", uri=True)
    print("\n### Query plans (realistic ledger)\n")
    print("| Read | Plan |\n| --- | --- |")
    for name, sql in PLANS.items():
        n = sql.count("?")
        detail = "; ".join(r[3] for r in con.execute(f"EXPLAIN QUERY PLAN {sql}", [None] * n))
        print(f"| {name} | {detail} |")
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
    root = tempfile.mkdtemp(prefix="seamlane-perf-")
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
        print(f"\nM1 max RSS: --version {max_rss(b, '--version'):.2f} MiB, "
              f"work list (empty) {max_rss(b, 'work', 'list'):.2f} MiB")
        parent, key = "bench/repo#1", "bench/repo#2"
        b.run("spec", "create", "--parent", parent, "--file", "-", stdin=spec_body(parent, [key]))
        hot(b, "empty", key)

        for suffix in ("", "-wal", "-shm"):
            if os.path.exists(b.db + suffix):
                os.remove(b.db + suffix)
            if os.path.exists(snapshot + suffix):
                shutil.copy(snapshot + suffix, b.db + suffix)
        hot(b, f"realistic ({items} items)", f"bench/repo#{200000 + items // 2}")
        handoffs = b.timed(lambda i: (["work", "handoff", str(deep[i % len(deep)])], handoff()))
        print(row(f"work handoff, lane with {DEEP_EVENTS}+ events", handoffs))
        print(f"\nM1 max RSS, realistic: work list {max_rss(b, 'work', 'list'):.2f} MiB, "
              f"work board {max_rss(b, 'work', 'board'):.2f} MiB")
        board = b.timed(lambda i: (["work", "board"], None), warm=2, samples=20)
        print(f"\nwork board, realistic: p50 {pct(board, 50):.2f} ms, "
              f"max {max(board):.2f} ms (20 samples)")
        plans(b)
        contention(b)
    finally:
        shutil.rmtree(root)


def stamp(path):
    """Modification times of the real ledger's directory and files, or None."""
    files = [path] + [os.path.join(path, f) for f in ("ledger.db", "ledger.db-wal")]
    return [os.stat(f).st_mtime_ns if os.path.exists(f) else None for f in files]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("binaries", nargs="+", help="the first one seeds the shared snapshot")
    ap.add_argument("--items", type=int, default=3000)
    a = ap.parse_args()
    real = os.path.expanduser("~/Library/Application Support/seamlane")
    real_before = stamp(real)
    print(f"python sqlite {sqlite3.sqlite_version} (query plans only)")
    seed_root = tempfile.mkdtemp(prefix="seamlane-seed-")
    try:
        # One seeded snapshot, checkpointed, so every binary reads the same data.
        s = Bench(a.binaries[0], seed_root)
        s.run("migrate")
        counts, secs = seed(s, a.items)
        deep = deepen(s)
        con = sqlite3.connect(s.db)
        con.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        counts["events with deep lanes"] = con.execute("SELECT count(*) FROM events").fetchone()[0]
        con.close()
        print(f"\nSeeded {counts} ({DEEP_LANES} deep lanes x {DEEP_EVENTS} checks) "
              f"through the CLI in {secs:.0f} s, ledger {os.path.getsize(s.db) / 2**20:.1f} MiB")
        for binary in a.binaries:
            measure(binary, s.db, a.items, deep)
    finally:
        shutil.rmtree(seed_root)
        assert stamp(real) == real_before, f"{real} changed during the run"


if __name__ == "__main__":
    main()

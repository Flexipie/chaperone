#!/usr/bin/env python3
"""R14 U1, U3, U4 and the GPUI U2, GPUI against egui on one seeded ledger.

    python3 -I spikes/gpui/measure.py <seed> <gpui-app> <egui-app>

Release binaries. Each is copied into a fresh dir under /private/tmp and run
from there, with HOME and XDG_DATA_HOME pointing there too, so a missed --db
can never reach a real ledger. `seed` writes the ledger of chaperone-ui's
U2 test. U1: one unmeasured launch each, then 10 `--first-frame` launches
each, alternated, wall time around the spawn. U3 and U4: each app opened
alone for 5 s, then its CPU time over 20 s and its RSS at the end. U2:
`seed u2` (commit to repaint request); egui's U2 is its own ignored test. Prints
markdown. Python stdlib only.
"""

import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

LAUNCHES = 10
SETTLE, IDLE = 5, 20


def cpu_secs(pid):
    """The process's CPU time, from ps's [[dd-]hh:]mm:ss.cc."""
    out = subprocess.run(["ps", "-o", "time=", "-p", str(pid)], capture_output=True, text=True)
    secs = 0.0
    for part in out.stdout.strip().split(":"):
        secs = secs * 60 + float(part)
    return secs


def rss_mib(pid):
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True)
    return int(out.stdout.strip()) / 1024


def main(seed, gpui, egui):
    root = tempfile.mkdtemp(prefix="gpui-budget-", dir="/private/tmp")
    env = {"PATH": "/usr/bin:/bin", "HOME": root, "XDG_DATA_HOME": root}
    bins = {}
    for name, path in [("seed", seed), ("gpui", gpui), ("egui", egui)]:
        bins[name] = os.path.join(root, name)
        shutil.copy2(path, bins[name])
    db = os.path.join(root, "ledger.db")
    subprocess.run([bins["seed"], db], check=True, env=env, capture_output=True)
    argv = {
        "gpui": [bins["gpui"], "--db", db],
        "egui": [bins["egui"], "--db", db, "--dotfiles", os.path.join(root, "dotfiles")],
    }
    print("| budget | GPUI | egui |\n|---|---|---|", flush=True)
    try:
        u1 = {app: [] for app in argv}
        for n in range(LAUNCHES + 1):
            for app, args in argv.items():
                start = time.perf_counter()
                subprocess.run([*args, "--first-frame"], check=True, env=env)
                if n:
                    u1[app].append((time.perf_counter() - start) * 1000)
        p50 = {a: statistics.median(t) for a, t in u1.items()}
        top = {a: max(t) for a, t in u1.items()}
        print(f"| U1 p50 (max), {LAUNCHES} launches | {p50['gpui']:.0f} ms ({top['gpui']:.0f}) "
              f"| {p50['egui']:.0f} ms ({top['egui']:.0f}) |", flush=True)
        idle = {}
        for app, args in argv.items():
            proc = subprocess.Popen(args, env=env)
            try:
                time.sleep(SETTLE)
                before = cpu_secs(proc.pid)
                time.sleep(IDLE)
                used = cpu_secs(proc.pid) - before
                idle[app] = (used / IDLE * 100, rss_mib(proc.pid))
            finally:
                proc.kill()
                proc.wait()
        print(f"| U3 CPU over {IDLE} s | {idle['gpui'][0]:.2f}% | {idle['egui'][0]:.2f}% |")
        print(f"| U4 RSS after {SETTLE + IDLE} s | {idle['gpui'][1]:.0f} MiB "
              f"| {idle['egui'][1]:.0f} MiB |", flush=True)
        subprocess.run([bins["seed"], "u2", db, bins["gpui"]], check=True, env=env)
    finally:
        shutil.rmtree(root)


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    main(*sys.argv[1:])

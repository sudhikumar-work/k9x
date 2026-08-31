#!/usr/bin/env python3
"""k9x vs k9s benchmark — LOCAL DUMMY CLUSTER ONLY (kind-k9x-test).

Safety: every invocation passes an explicit context flag and the script
aborts unless the target API server resolves to 127.0.0.1/localhost.
Stage/prod clusters are never contacted by this script.

Usage: python3 scripts/bench.py [--runs N]
Writes tests/BENCHMARK.md.
"""
import json
import os
import pty
import re
import select
import signal
import statistics
import struct
import subprocess
import sys
import termios
import time
import fcntl

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
K9X = os.path.join(REPO, "target", "release", "k9x")
K9S = "/opt/homebrew/bin/k9s"
CTX = "kind-k9x-test"
NS = "demo"

RUNS_EXEC = 30
RUNS_TUI = 5


def die(msg):
    print(f"ABORT: {msg}")
    sys.exit(1)


def guard_dummy_cluster():
    """abort unless the explicit context points at localhost"""
    out = subprocess.run(
        ["kubectl", "--context", CTX, "cluster-info"],
        capture_output=True, text=True, timeout=20,
    )
    m = re.search(r"https://[^\s]+", out.stdout)
    if not m:
        die(f"cannot resolve cluster endpoint for {CTX}")
    host = re.sub(r"https?://", "", m.group(0)).split(":")[0]
    if host not in ("127.0.0.1", "localhost"):
        die(f"{CTX} does not point at localhost (got {host}) — refusing to benchmark")
    print(f"guard ok: {CTX} → {m.group(0)} (local dummy cluster)")


def timed_cmd(argv, n):
    times = []
    for _ in range(n):
        t0 = time.perf_counter()
        subprocess.run(argv, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        times.append((time.perf_counter() - t0) * 1000)
    return sorted(times)


def pct(sorted_ms, p):
    i = min(len(sorted_ms) - 1, int(round(p / 100 * len(sorted_ms))) )
    return sorted_ms[i]


class TuiProbe:
    """launch a TUI on a pty and measure first-frame / first-data latency"""

    def __init__(self, argv, cols=140, rows=45):
        self.argv = argv
        self.cols, self.rows = cols, rows

    def run(self):
        import pyte
        pid, fd = pty.fork()
        if pid == 0:
            os.execvp(self.argv[0], self.argv)
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HH", self.rows, self.cols))
        screen = pyte.Screen(self.cols, self.rows)
        stream = pyte.ByteStream(screen)

        t0 = time.perf_counter()
        t_frame = t_data = None
        buf = b""

        def feed(chunk):
            chunk = re.sub(rb"\x1b\[\?\d+[hl]", b"", chunk)
            chunk = re.sub(rb"\x1b\[>\d+m", b"", chunk)
            try:
                stream.feed(chunk)
            except Exception:
                pass

        deadline = t0 + 30
        while time.perf_counter() < deadline:
            r, _, _ = select.select([fd], [], [], 0.04)
            if r:
                try:
                    chunk = os.read(fd, 65536)
                except OSError:
                    break
                if not chunk:
                    break
                buf += chunk
                feed(chunk)
                txt = "\n".join(screen.display)
                if t_frame is None and "NAME" in txt and "READY" in txt:
                    t_frame = time.perf_counter() - t0
                if t_frame is not None and t_data is None and txt.count("Running") >= 1:
                    t_data = time.perf_counter() - t0
                    break
        rss_kb = None
        try:
            time.sleep(4.0)  # settle before memory sample
            # keep draining so the app can paint while we sample
            r, _, _ = select.select([fd], [], [], 0.05)
            if r:
                try:
                    feed(os.read(fd, 65536))
                except OSError:
                    pass
            out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True)
            if out.stdout.strip():
                rss_kb = int(out.stdout.strip())
        finally:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            try:
                os.close(fd)
            except OSError:
                pass
            try:
                os.waitpid(pid, 0)
            except ChildProcessError:
                pass
        return t_frame, t_data, rss_kb


def bench_tui(name, argv):
    frames, datas, rss = [], [], []
    print(f"  {name}: ", end="", flush=True)
    for i in range(RUNS_TUI):
        tf, td, rk = TuiProbe(argv).run()
        if tf is not None:
            frames.append(tf * 1000)
        if td is not None:
            datas.append(td * 1000)
        if rk:
            rss.append(rk)
        print(".", end="", flush=True)
    print(" done")
    med = lambda xs: statistics.median(xs) if xs else None
    return {
        "ttff_med": med(frames), "tffd_med": med(datas),
        "rss_med_kb": med(rss),
        "ok": len(frames) == RUNS_TUI,
    }


def fmt(ms):
    return f"{ms:.1f}ms" if ms is not None else "—"


def main():
    runs_exec = RUNS_EXEC
    if "--runs" in sys.argv:
        runs_exec = int(sys.argv[sys.argv.index("--runs") + 1])

    for b in (K9X, K9S):
        if not os.path.exists(b):
            die(f"binary missing: {b}")
    guard_dummy_cluster()

    npods = subprocess.run(
        ["kubectl", "--context", CTX, "-n", NS, "get", "po", "--no-headers"],
        capture_output=True, text=True,
    ).stdout.strip().count("\n") + 1
    print(f"sample size: {npods} pods in ns/{NS}")

    k9x_ver = subprocess.run([K9X, "--version"], capture_output=True, text=True).stdout.strip()
    k9s_out = subprocess.run([K9S, "version", "--short"], capture_output=True, text=True).stdout
    mver = re.search(r"Version\s+(\S+)", k9s_out)
    k9s_ver = mver.group(1) if mver else "unknown"

    print(f"\n[1/3] process exec overhead ({runs_exec} runs each, no network)")
    x = timed_cmd([K9X, "--version"], runs_exec)
    s = timed_cmd([K9S, "version", "--short"], runs_exec)
    exec_row = {
        "k9x": {"p50": pct(x, 50), "p95": pct(x, 95)},
        "k9s": {"p50": pct(s, 50), "p95": pct(s, 95)},
    }
    print(f"  k9x p50={exec_row['k9x']['p50']:.1f}ms p95={exec_row['k9x']['p95']:.1f}ms")
    print(f"  k9s p50={exec_row['k9s']['p50']:.1f}ms p95={exec_row['k9s']['p95']:.1f}ms")

    print(f"\n[2/3] agent one-shot list (network included)")
    t0 = time.perf_counter()
    subprocess.run([K9X, "-x", CTX, "ls", "po", "-n", NS, "-o", "name"], stdout=subprocess.DEVNULL)
    agent_ms = (time.perf_counter() - t0) * 1000
    print(f"  k9x ls po -o name: {agent_ms:.1f}ms (informational — k9s has no CLI mode)")

    print(f"\n[3/3] TUI on dummy cluster ({RUNS_TUI} launches each: first-frame / first-data / idle RSS)")
    xr = bench_tui("k9x", [K9X, "-x", CTX, "-n", NS, "po"])
    sr = bench_tui("k9s", [K9S, "--context", CTX, "-n", NS, "-c", "po"])

    sz_x = os.path.getsize(K9X) / 1024 / 1024
    sz_s = os.path.getsize(K9S) / 1024 / 1024

    speedup_exec = exec_row["k9s"]["p50"] / exec_row["k9x"]["p50"]
    ttff_ratio = sr["ttff_med"] / xr["ttff_med"] if xr["ttff_med"] and sr["ttff_med"] else None
    tffd_ratio = sr["tffd_med"] / xr["tffd_med"] if xr["tffd_med"] and sr["tffd_med"] else None
    rss_ratio = sr["rss_med_kb"] / xr["rss_med_kb"] if xr["rss_med_kb"] and sr["rss_med_kb"] else None

    def kb(v):
        return f"{v/1024:.1f}MB" if v else "—"

    report = f"""# k9x vs k9s Benchmark

**Date:** {time.strftime('%Y-%m-%d %H:%M')} · **Machine:** macOS (Apple Silicon, local laptop)
**Cluster:** `{CTX}` (local kind dummy, 127.0.0.1) — **never stage/prod**
**Workload:** ns/{NS}, {npods} pods · **k9x** {k9x_ver} · **k9s** {k9s_ver}

> Safety: every launch pinned the context explicitly (`k9x -x {CTX}`, `k9s --context {CTX}`)
> and the harness aborts unless the endpoint resolves to 127.0.0.1/localhost.

## Results

| Metric | k9x | k9s | Advantage |
|---|---|---|---|
| Process exec p50 (no network) | **{fmt(exec_row['k9x']['p50'])}** | {fmt(exec_row['k9s']['p50'])} | ~{speedup_exec:.1f}× |
| Process exec p95 | {fmt(exec_row['k9x']['p95'])} | {fmt(exec_row['k9s']['p95'])} | — |
| Time-to-first-frame (median) | **{fmt(xr['ttff_med'])}** | {fmt(sr['ttff_med'])} | {'~%.1f×' % ttff_ratio if ttff_ratio else '—'} |
| Time-to-first-data (median) | **{fmt(xr['tffd_med'])}** | {fmt(sr['tffd_med'])} | {'~%.1f×' % tffd_ratio if tffd_ratio else '—'} |
| Idle RSS (median) | **{kb(xr['rss_med_kb'])}** | {kb(sr['rss_med_kb'])} | {'~%.1f×' % rss_ratio if rss_ratio else '—'} smaller |
| Binary size | **{sz_x:.1f}MB** | {sz_s:.1f}MB | ~{sz_s/sz_x:.0f}× smaller |

Informational: `k9x ls po -o name` full round-trip incl. API call: **{agent_ms:.0f}ms** (k9s has no CLI mode to compare).
First-frame = table chrome painted; first-data = actual pod rows visible. RSS sampled ~4s after data, median of {RUNS_TUI} runs.

## Methodology

- exec overhead: `k9x --version` vs `k9s version --short`, {runs_exec} sequential spawns each, perf_counter timing
- TUI metrics: pty + terminal emulator replay, {RUNS_TUI} fresh launches per tool, medians
- both tools pointed at the identical namespace/view (`po` in ns/{NS}) on the same node
"""
    with open(os.path.join(REPO, "tests", "BENCHMARK.md"), "w") as f:
        f.write(report)
    print("\n" + report)
    print("report → tests/BENCHMARK.md")


if __name__ == "__main__":
    main()

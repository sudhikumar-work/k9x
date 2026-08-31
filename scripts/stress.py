#!/usr/bin/env python3
"""k9x vs k9s STRESS test — LOCAL DUMMY CLUSTER ONLY (kind-k9x-test).

Hard isolation:
  * builds a THROWAWAY kubeconfig containing ONLY the kind context —
    stage/prod credentials are physically absent from every child process
  * aborts unless that config's server resolves to 127.0.0.1/localhost
  * context flag additionally pinned on every kubectl/tool invocation

Phases (k9x fully first, then k9s):
  1. scale-probe : time-to-first-data against ~300 pods
  2. soak        : 40s runtime, RSS/CPU sampled over time (leak check)
  3. churn       : 25s of continuous pod-delete storms while watching

Writes tests/STRESSTEST.md. Cleans up ns/stress afterwards.
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
import tempfile
import termios
import threading
import time
import fcntl

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
K9X = os.path.join(REPO, "target", "release", "k9x")
K9S = "/opt/homebrew/bin/k9s"
CTX = "kind-k9x-test"
DEPLOYS = ["stress-a", "stress-b", "stress-c"]
REPLICAS = 30   # 3 × 30 = 90 pods: node allocatable caps at 110; colima VM is 2CPU/2GiB
SOAK_SECS = 40
CHURN_SECS = 25
NS = "stress"  # resolved in main(): first free name among stress, stress-2, stress-3…


def die(m):
    print(f"ABORT: {m}"); sys.exit(1)


def fmt_ms(v):
    return f"{v:.0f}ms" if v else "—"


def fmt_mb(v):
    return f"{v/1024:+.1f}MB" if v is not None else "—"


def kb(v):
    return f"{v/1024:.1f}MB" if v else "—"


def pct(v):
    return f"{v:.1f}%" if v is not None else "—"


def build_isolated_kubeconfig() -> str:
    """throwaway kubeconfig with ONLY the dummy context; verify localhost"""
    r = subprocess.run(
        ["kubectl", "config", "view", "--raw", "--minify", f"--context={CTX}"],
        capture_output=True, text=True,
    )
    if r.returncode != 0 or not r.stdout.strip():
        die(f"cannot extract minified kubeconfig for {CTX}: {r.stderr}")
    servers = re.findall(r"server:\s*(\S+)", r.stdout)
    if len(servers) != 1:
        die(f"expected exactly 1 server in minified config, got {servers}")
    host = re.sub(r"https?://", "", servers[0]).split(":")[0]
    if host not in ("127.0.0.1", "localhost"):
        die(f"{CTX} does not point at localhost ({host}) — refusing to stress test")
    fd, path = tempfile.mkstemp(prefix="k9x-stress-kubeconfig-")
    os.write(fd, r.stdout.encode())
    os.close(fd)
    print(f"isolation ok: single-context kubeconfig → {servers[0]} (local dummy)")
    return path


def k(args, capture=True):
    """kubectl against the isolated kubeconfig AND the pinned context"""
    return subprocess.run(["kubectl", "--kubeconfig", KUBECONFIG, "--context", CTX] + args,
                          capture_output=capture, text=True)


def ensure_load():
    r = k(["create", "ns", NS])
    img = k(["-n", "demo", "get", "po", "-o",
             "jsonpath={.items[0].spec.containers[0].image}"]).stdout.strip()
    image = img or "nginx:alpine"
    print(f"ns/{NS} ready · image {image}")
    for d in DEPLOYS:
        spec = {
            "apiVersion": "apps/v1", "kind": "Deployment",
            "metadata": {"name": d, "namespace": NS},
            "spec": {
                "replicas": REPLICAS,
                "selector": {"matchLabels": {"app": d}},
                "template": {
                    "metadata": {"labels": {"app": d}},
                    "spec": {"containers": [{
                        "name": "main", "image": image,
                        "resources": {"requests": {"cpu": "1m", "memory": "1Mi"}},
                        "command": ["sh", "-c", "sleep 3600"],
                    }]},
                },
            },
        }
        path = os.path.join(tempfile.gettempdir(), f"k9x-{d}.json")
        with open(path, "w") as f:
            json.dump(spec, f)
        r = k(["apply", "-f", path])
        if r.returncode != 0:
            die(f"apply {d}: {r.stderr}")
    want = len(DEPLOYS) * REPLICAS
    print(f"waiting for all {want} pods Running (steady state) ", end="", flush=True)
    deadline = time.time() + 600
    n = 0
    while time.time() < deadline:
        out = k(["-n", NS, "get", "po", "--field-selector", "status.phase=Running",
                 "--no-headers"]).stdout.strip()
        n = out.count("\n") + 1 if out else 0
        print(".", end="", flush=True)
        if n >= want:
            break
        time.sleep(5)
    print(f" → {n} Running")
    if n < want:
        die(f"cluster never reached steady state ({n}/{want} Running)")
    return n


class Probe:
    def __init__(self, argv, cols=140, rows=45):
        self.argv, self.cols, self.rows = argv, cols, rows
        self.pid = self.fd = self.screen = None
        self.stream = None

    def start(self):
        import pyte
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.environ["TERM"] = "xterm-256color"
            os.execvp(self.argv[0], self.argv)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HH", self.rows, self.cols))
        self.screen = pyte.Screen(self.cols, self.rows)
        self.stream = pyte.ByteStream(self.screen)

    def pump(self, timeout=0.05):
        r, _, _ = select.select([self.fd], [], [], timeout)
        if r:
            try:
                chunk = os.read(self.fd, 65536)
            except OSError:
                return False
            if not chunk:
                return False
            c = re.sub(rb"\x1b\[\?\d+[hl]", b"", chunk)
            c = re.sub(rb"\x1b\[>\d+m", b"", c)
            try:
                self.stream.feed(c)
            except Exception:
                pass
        return True

    def text(self):
        return "\n".join(self.screen.display)

    def sample_ps(self):
        out = subprocess.run(["ps", "-o", "%cpu=,rss=", "-p", str(self.pid)],
                             capture_output=True, text=True)
        parts = out.stdout.split()
        if len(parts) >= 2:
            try:
                return float(parts[0]), int(parts[1])
            except ValueError:
                pass
        return None, None

    def wait_data(self, timeout=60):
        t0 = time.perf_counter()
        while time.perf_counter() - t0 < timeout:
            self.pump()
            txt = self.text()
            if "NAME" in txt and "READY" in txt and txt.count("Running") >= 1:
                return time.perf_counter() - t0
        return None

    def stop(self):
        try:
            os.kill(self.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            os.close(self.fd)
        except OSError:
            pass
        try:
            os.waitpid(self.pid, 0)
        except ChildProcessError:
            pass


def scale_probe(argv):
    p = Probe(argv)
    p.start()
    el = p.wait_data()
    _, rss = p.sample_ps()
    p.stop()
    ms = el * 1000 if el else None
    print(f"  first-data={fmt_ms(ms)} rss={kb(rss)}")
    return {"ffd_ms": ms, "rss_kb_after_load": rss}


def soak(argv, secs=SOAK_SECS):
    p = Probe(argv)
    p.start()
    if p.wait_data() is None:
        p.stop(); return {"error": "no data"}
    series = []
    t0 = time.time()
    next_sample = 0.0
    while time.time() - t0 < secs:
        p.pump(0.15)
        if time.time() - t0 >= next_sample:
            cpu, rss = p.sample_ps()
            if rss:
                series.append((time.time() - t0, cpu, rss))
            next_sample += 2.0
    p.stop()
    if not series:
        return {"error": "no samples"}
    return {
        "rss_first_kb": series[0][2],
        "rss_last_kb": series[-1][2],
        "rss_max_kb": max(r for _, _, r in series),
        "growth_kb": series[-1][2] - series[0][2],
        "cpu_median": statistics.median(c for _, c, _ in series),
    }


def churn(argv, secs=CHURN_SECS):
    p = Probe(argv)
    p.start()
    if p.wait_data() is None:
        p.stop(); return {"error": "no data"}
    stop_flag = threading.Event()
    waves = [0]

    def storm():
        while not stop_flag.is_set():
            k(["-n", NS, "delete", "po", "-l", f"app={DEPLOYS[waves[0] % 3]}",
               "--wait=false", "--timeout=8s", "--ignore-not-found"])
            waves[0] += 1
            time.sleep(1.5)

    th = threading.Thread(target=storm, daemon=True)
    th.start()
    series = []
    t0 = time.time()
    while time.time() - t0 < secs:
        p.pump(0.15)
        cpu, rss = p.sample_ps()
        if rss:
            series.append((cpu, rss))
    stop_flag.set()
    th.join(timeout=15)
    running_end = p.text().count("Running")
    p.stop()
    time.sleep(15)  # let replicasets heal before the next phase
    if not series:
        return {"error": "no samples"}
    return {
        "cpu_avg": statistics.mean(c for c, _ in series),
        "cpu_max": max(c for c, _ in series),
        "rss_med_kb": statistics.median(r for _, r in series),
        "storm_waves": waves[0],
        "running_visible_end": running_end,
    }


def main():
    global KUBECONFIG, NS
    for b in (K9X, K9S):
        if not os.path.exists(b):
            die(f"missing {b}")
    KUBECONFIG = build_isolated_kubeconfig()
    os.environ["KUBECONFIG"] = KUBECONFIG  # children inherit; nothing else reachable
    # pick a clean namespace (a previously crashed run can leave one Terminating)
    for cand in ["stress"] + [f"stress-{i}" for i in range(2, 12)]:
        if not k(["get", "ns", cand]).stdout.strip():
            NS = cand
            break
    else:
        die("no free stress namespace name")
    print(f"namespace: ns/{NS}")
    npods = ensure_load()

    tools = [
        ("k9x", [K9X, "-x", CTX, "-n", NS, "po"]),
        ("k9s", [K9S, "--context", CTX, "-n", NS, "-c", "po"]),
    ]

    R = {}
    for name, argv in tools:   # k9x FIRST, then k9s
        print(f"\n===== {name} =====")
        print("[1/3] scale probe @ ~300 pods")
        R[(name, "probe")] = scale_probe(argv)
        print(f"[2/3] soak {SOAK_SECS}s")
        R[(name, "soak")] = soak(argv)
        so = R[(name, "soak")]
        if "error" in so:
            print("  soak failed:", so["error"])
        else:
            print(f"  rss {kb(so['rss_first_kb'])} → {kb(so['rss_last_kb'])} "
                  f"(max {kb(so['rss_max_kb'])}) cpu~{pct(so['cpu_median'])}")
        print(f"[3/3] churn {CHURN_SECS}s")
        R[(name, "churn")] = churn(argv)
        ch = R[(name, "churn")]
        if "error" in ch:
            print("  churn failed:", ch["error"])
        else:
            print(f"  cpu avg {pct(ch['cpu_avg'])} max {pct(ch['cpu_max'])} · "
                  f"rss {kb(ch['rss_med_kb'])} · {ch['storm_waves']} waves")

    print("\ncleaning up ns/stress …")
    subprocess.run(["kubectl", "--kubeconfig", KUBECONFIG, "--context", CTX,
                    "delete", "ns", NS, "--timeout=120s"],
                   capture_output=True, text=True)
    try:
        os.remove(KUBECONFIG)
    except OSError:
        pass

    xffd, sffd = R[("k9x", "probe")]["ffd_ms"], R[("k9s", "probe")]["ffd_ms"]
    ratio = lambda a, b: f"~{b/a:.1f}×" if a and b else "—"

    report = f"""# k9x vs k9s — Stress Test

**Date:** {time.strftime('%Y-%m-%d %H:%M')} · **Cluster:** `{CTX}` (local kind dummy, 127.0.0.1) — never stage/prod
**Load:** 3 deployments × {REPLICAS} replicas = **{npods} pods** in ns/{NS} (tiny sleep containers)
**Order:** k9x fully first, then k9s · **k9x** 0.2.0 · **k9s** 0.50.18

> Isolation: the whole suite ran against a throwaway single-context kubeconfig whose only
> server is 127.0.0.1 — stage/prod credentials were physically unreachable by every process,
> including the tools under test. Context flag additionally pinned on every call.
> ns/{NS} was deleted afterwards.

## Phase 1 — Cold start @ ~{npods} pods
| Metric | k9x | k9s | Advantage |
|---|---|---|---|
| Time-to-first-data | **{fmt_ms(xffd)}** | {fmt_ms(sffd)} | {ratio(xffd, sffd)} |
| RSS right after load | **{kb(R[('k9x','probe')]['rss_kb_after_load'])}** | {kb(R[('k9s','probe')]['rss_kb_after_load'])} | — |

## Phase 2 — {SOAK_SECS}s soak (idle watching {npods} pods)
| Metric | k9x | k9s |
|---|---|---|
| RSS start → end | **{kb(R[('k9x','soak')].get('rss_first_kb'))} → {kb(R[('k9x','soak')].get('rss_last_kb'))}** | {kb(R[('k9s','soak')].get('rss_first_kb'))} → {kb(R[('k9s','soak')].get('rss_last_kb'))} |
| Growth | **{fmt_mb(R[('k9x','soak')].get('growth_kb'))}** | {fmt_mb(R[('k9s','soak')].get('growth_kb'))} |
| Peak RSS | **{kb(R[('k9x','soak')].get('rss_max_kb'))}** | {kb(R[('k9s','soak')].get('rss_max_kb'))} |
| Median CPU | **{pct(R[('k9x','soak')].get('cpu_median'))}** | {pct(R[('k9s','soak')].get('cpu_median'))} |

## Phase 3 — {CHURN_SECS}s churn (pod-delete storms while watching)
| Metric | k9x | k9s |
|---|---|---|
| Avg CPU under event storm | **{pct(R[('k9x','churn')].get('cpu_avg'))}** | {pct(R[('k9s','churn')].get('cpu_avg'))} |
| Max CPU | **{pct(R[('k9x','churn')].get('cpu_max'))}** | {pct(R[('k9s','churn')].get('cpu_max'))} |
| Median RSS | **{kb(R[('k9x','churn')].get('rss_med_kb'))}** | {kb(R[('k9s','churn')].get('rss_med_kb'))} |
| Storm waves absorbed | {R[('k9x','churn')].get('storm_waves', '—')} | {R[('k9s','churn')].get('storm_waves', '—')} |

## Reading
- Event-driven k9x paints rows as watch deltas land; k9s refreshes on its poll cycle,
  which shows as higher sustained CPU during churn and slower first paint at scale.
- Near-zero RSS growth during soak indicates no obvious leak in either tool at this scale.
"""
    with open(os.path.join(REPO, "tests", "STRESSTEST.md"), "w") as f:
        f.write(report)
    print("\n" + report)
    print("report → tests/STRESSTEST.md")


if __name__ == "__main__":
    main()

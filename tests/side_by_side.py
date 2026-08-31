"""Side-by-side k9s vs k9x comparison on the local kind cluster ONLY.
Usage: python3 tests/side_by_side.py [scenario-substring ...]
"""
import pty, os, time, select, fcntl, termios, struct, re, sys, subprocess
import pyte

K9S = "/opt/homebrew/bin/k9s"
K9X = "/Users/sudheeshkumar.surendran/Simply Jet/Repos/k9x/target/release/k9x"
CTX = "kind-k10s-test"
NS = "demo"

def run_tool(binpath, args, keys_at, total=13.0, cols=170, rows=45):
    """keys_at: list of (t, bytes). Returns (final_text, ttfd_seconds|None, raw_bytes_len)."""
    pid, fd = pty.fork()
    if pid == 0:
        os.execv(binpath, [os.path.basename(binpath)] + args)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    screen = pyte.Screen(cols, rows)
    stream = pyte.ByteStream(screen)
    buf = bytearray()
    t0 = time.time()
    ttfd = None
    pending = sorted(keys_at, key=lambda x: x[0])
    # regex that signals "real data rendered" for this tool
    datapat = re.compile(rb"(web-|api-|Running|NAME)")
    while time.time() - t0 < total:
        if pending and time.time() - t0 > pending[0][0]:
            _, ks = pending.pop(0)
            for k in ks:
                os.write(fd, k)
                time.sleep(0.25)
        r, _, _ = select.select([fd], [], [], 0.25)
        if r:
            try:
                d = os.read(fd, 65536)
            except OSError:
                break
            if not d:
                break
            buf += d
            if ttfd is None and datapat.search(d):
                ttfd = time.time() - t0
            try:
                stream.feed(d)
            except Exception:
                pass
    time.sleep(0.5)
    try:
        os.write(fd, b"\x03")
        os.close(fd)
    except OSError:
        pass
    try:
        pid2, status, ru = os.wait4(pid, 0)
        rss_kb = ru.ru_maxrss
    except ChildProcessError:
        rss_kb = -1
    lines = ["".join(r).rstrip() for r in screen.display]
    text = "\n".join(lines)
    return text, ttfd, rss_kb

ONLY = sys.argv[1:]
rows_out = []

def record(name, k9s_ok, k9x_ok, note=""):
    if ONLY and not any(o in name for o in ONLY):
        return
    rows_out.append((name, k9s_ok, k9x_ok, note))
    def m(ok): return "PASS" if ok else "FAIL"
    print(f"{name:38} k9s:{m(k9s_ok)}  k9x:{m(k9x_ok)}  {note}")

def base_args(tool):
    common = ["--context", CTX]
    return common

# ---------- S1 pods ----------
t9, l9, r9 = run_tool(K9S, ["--context", CTX, "-n", NS], [], total=11)
t1, l1, r1 = run_tool(K9X, ["-x", CTX, "-n", NS], [], total=11)
record("S01 pods-render", ("web-" in t9 and "Running" in t9), ("web-" in t1 and b"Running" in t1.encode()), f"ttfd k9s={l9 and round(l9,2)}s k9x={l1 and round(l1,2)}s")

# ---------- S2 all-namespaces ----------
t9, l9, _ = run_tool(K9S, ["--context", CTX, "-A"], [], total=11)
t1, l1, _ = run_tool(K9X, ["-x", CTX, "-A"], [], total=11)
record("S02 all-namespaces", ("argocd" in t9 or "coredns" in t9), ("coredns" in t1 or "etcd-" in t1 or b"ns:all".decode() in t1))

# ---------- S3 deploy view ----------
t9, _, _ = run_tool(K9S, ["--context", CTX, "-n", NS], [(6.5, [b":"]), (7.2, [b"d", b"e", b"p", b"l", b"o", b"y"]), (8.8, [b"\r"])], total=12)
t1, _, _ = run_tool(K9X, ["deploy", "-n", NS], [], total=11)
record("S03 deployments-view", ("READY" in t9.upper() and ("web" in t9 or "api" in t9)), ("READY" in t1 and ("web" in t1 or "api" in t1)))

# ---------- S4 nodes ----------
t9, _, _ = run_tool(K9S, ["--context", CTX], [(6.5, [b":"]), (7.2, [b"n", b"o"]), (8.2, [b"\r"])], total=12)
t1, _, _ = run_tool(K9X, ["no"], [], total=10)
record("S04 nodes-view", ("Ready" in t9 or "control-plane" in t9), ("Ready" in t1 or "control-plane" in t1))

# ---------- S5 configmaps ----------
t9, _, _ = run_tool(K9S, ["--context", CTX, "-n", NS], [(6.5, [b":"]), (7.2, [b"c", b"m"]), (8.2, [b"\r"])], total=12)
t1, _, _ = run_tool(K9X, ["cm", "-n", NS], [], total=10)
record("S05 configmaps", ("app-config" in t9), ("app-config" in t1))

# ---------- S6 secrets ----------
t9, _, _ = run_tool(K9S, ["--context", CTX, "-n", NS], [(6.5, [b":"]), (7.2, [b"s", b"e", b"c"]), (8.4, [b"\r"])], total=12)
t1, _, _ = run_tool(K9X, ["sec", "-n", NS], [], total=10)
record("S06 secrets", ("app-sec" in t9 or "sh.helm" in t9), ("app-sec" in t1 or "sh.helm" in t1))

# ---------- S7 cronjobs ----------
t9, _, _ = run_tool(K9S, ["--context", CTX, "-n", NS], [(6.5, [b":"]), (7.2, [b"c", b"j"]), (8.2, [b"\r"])], total=12)
t1, _, _ = run_tool(K9X, ["cj", "-n", NS], [], total=10)
record("S07 cronjobs", ("ticker" in t9), ("ticker" in t1))

# ---------- S8 filter ----------
t9, _, _ = run_tool(K9S, ["--context", CTX, "-n", NS], [(7.0, [b"/"]), (8.0, [b"w", b"e", b"b"])], total=12)
t1, _, _ = run_tool(K9X, ["po", "-n", NS], [(7.0, [b"/"]), (8.0, [b"w", b"e", b"b"])], total=12)
ok9 = "web-" in t9
record("S08 filter-/web", ok9, ("web-" in t1 and "pods(2)" in t1.replace(" ", "")))

# ---------- S9 yaml ----------
t9, _, _ = run_tool(K9S, ["--context", CTX, "-n", NS], [(7.0, [b"y"])], total=12)
t1, _, _ = run_tool(K9X, ["po", "-n", NS], [(7.0, [b"y"])], total=11)
record("S09 yaml-view", ("apiVersion" in t9), ("apiVersion" in t1))

# ---------- S10 describe ----------
t9, _, _ = run_tool(K9S, ["--context", CTX, "-n", NS], [(7.0, [b"d"])], total=13)
t1, _, _ = run_tool(K9X, ["po", "-n", NS], [(7.0, [b"d"])], total=11)
record("S10 describe", ("Name:" in t9 and ("Events:" in t9 or "Container" in t9)), ("Name:" in t1 and "Labels:" in t1))

# ---------- S11 logs ----------
t9, _, _ = run_tool(K9S, ["--context", CTX, "-n", NS], [(7.0, [b"l"]), (8.2, [b"\r"])], total=13)
t1, _, _ = run_tool(K9X, ["po", "-n", NS], [(7.0, [b"l"]), (8.2, [b"\r"])], total=12)
record("S11 logs-open", ("logs" in t9.lower()), ("logs demo/" in t1 or "[follow]" in t1 or "docker-entrypoint" in t1))

# ---------- S12 ctx ----------
t9, _, _ = run_tool(K9S, ["--context", CTX, "-n", NS], [(7.0, [b":"]), (7.7, [b"c", b"t", b"x"]), (8.9, [b"\r"])], total=12)
t1, _, _ = run_tool(K9X, ["po", "-n", NS], [(7.0, [b":", b"c", b"t", b"x"]), (8.9, [b"\r"])], total=12)
record("S12 contexts-list", ("kind-k10s-test" in t9), ("kind-k10s-test" in t1))

# ---------- S13 helm ----------
t9h, _, _ = run_tool(K9S, ["--context", CTX, "-n", NS], [(7.0, [b":"]), (7.7, [b"h", b"e", b"l", b"m"]), (9.2, [b"\r"])], total=13)
t1h, _, _ = run_tool(K9X, ["po", "-n", NS], [(7.0, [b":", b"h", b"e", b"l", b"m"]), (8.9, [b"\r"])], total=12)
ok9h = ("myrel" in t9h.lower() or "demo" in t9h.lower() and "helm" in t9h.lower())
record("S13 helm-releases", ok9h, ("myrel" in t1h))

# ---------- S14 pulse ----------
t9p, _, _ = run_tool(K9S, ["--context", CTX, "-n", NS], [(7.0, [b":"]), (7.7, [b"p", b"u", b"l", b"s", b"e"]), (9.4, [b"\r"])], total=13)
t1p, _, _ = run_tool(K9X, ["po", "-n", NS], [(7.0, [b":", b"p", b"u", b"l", b"s", b"e"]), (8.9, [b"\r"])], total=12)
record("S14 pulse", len(t9p.strip()) > 100, ("POD" in t1p.upper() and "DEPLOYMENT" in t1p.upper()))

# ---------- perf: version/exec + rss on pods ----------
import subprocess as sp
def bench_exec(cmd, n=7):
    ts = []
    for _ in range(n):
        t = time.perf_counter()
        sp.run(cmd, capture_output=True)
        ts.append((time.perf_counter() - t) * 1000)
    ts.sort()
    return ts[len(ts) // 2]
v9 = bench_exec([K9S, "version", "--short"])
v1 = bench_exec([K9X, "--version"])
b9 = os.path.getsize(K9S) // 1024 // 1024
b1 = os.path.getsize(K9X) // 1024 // 1024
print("\n==== PERF ====")
print(f"exec p50      k9s={v9:.1f}ms   k9x={v1:.1f}ms   ({v9/max(v1,0.1):.1f}x)")
print(f"binary        k9s={b9}MB       k9x={b1}MB")
print(f"rss pods-view k9s={r9//1048576}MB   k9x={r1//1048576}MB")

print("\n==== SUMMARY ====")
fails = [r for r in rows_out if not (r[1] and r[2])]
for name, a, b, note in rows_out:
    print(("OK  " if (a and b) else "DIFF") + f" {name:36} k9s={'Y' if a else '-'} k9x={'Y' if b else '-'} {note}")
print(f"\n{len(rows_out)-len(fails)}/{len(rows_out)} scenarios match; {len(fails)} differ")

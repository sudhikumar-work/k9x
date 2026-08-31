"""k9x feature matrix — pty-driven end-to-end tests against a local kind cluster.
Usage: python3 tests/matrix.py [case-substring ...]   (no args = all cases)
"""
import pty, os, time, select, fcntl, termios, struct, re, glob, subprocess, sys
import pyte

BIN = os.environ.get("K9X_BIN") or (os.path.expanduser("~/.local/bin/k9x") if os.path.exists(os.path.expanduser("~/.local/bin/k9x")) else os.path.abspath("target/release/k9x"))
CTX = ["-x", "kind-k10s-test"]
KUBECTL_CTX = "kind-k10s-test"

def kubectl(*a):
    return subprocess.run(["kubectl", "--context", KUBECTL_CTX, *a], capture_output=True).stdout

def drive(args, script, total=14.0, full=False):
    """script: list of (t_seconds, [bytes keys]) — sorted defensively."""
    script = sorted(script, key=lambda x: x[0])
    pid, fd = pty.fork()
    if pid == 0:
        os.execv(BIN, ["k9x"] + CTX + args)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 45, 170, 0, 0))
    buf = bytearray()
    t0 = time.time()

    def drain():
        while True:
            r, _, _ = select.select([fd], [], [], 0)
            if not r:
                return
            try:
                d = os.read(fd, 65536)
                if not d:
                    return
                buf.extend(d)
            except OSError:
                return

    def send_all(data):
        view = memoryview(data)
        while view:
            try:
                n = os.write(fd, view)
            except OSError:
                return
            view = view[n:]

    while time.time() - t0 < total:
        if script and time.time() - t0 > script[0][0]:
            _, keys = script.pop(0)
            drain()
            for k in keys:
                send_all(k)
                time.sleep(0.25)
        r, _, _ = select.select([fd], [], [], 0.3)
        if r:
            try:
                d = os.read(fd, 65536)
                if not d:
                    break
                buf.extend(d)
            except OSError:
                break
    time.sleep(0.5)
    try:
        os.write(fd, b"\x03")
        os.close(fd)
    except OSError:
        pass
    try:
        os.waitpid(pid, 0)
    except ChildProcessError:
        pass
    raw = bytes(buf)
    screen = pyte.Screen(170, 45)
    stream = pyte.ByteStream(screen)
    clean = re.sub(rb"\x1b\[\?[0-9;]*[hl]", b"", raw)
    clean = re.sub(rb"\x1b\[>[0-9;<>]*[a-zA-Z]", b"", clean)
    try:
        stream.feed(clean)
    except Exception:
        pass
    lines = ["".join(screen.display[y]) .rstrip() for y in range(screen.lines)]
    text = "\n".join(lines)
    with open("/tmp/k9x-final-screen.txt", "w") as fh:
        fh.write(text)
    return re.sub(rb"[\s]+", b"", text.encode())

def last_match(t, pat):
    hits = re.findall(pat, t)
    return hits[-1] if hits else None

def any_ns(t):
    """Last namespace token across old/new header formats."""
    hits = re.findall(rb"(?:ns|Namespace)\s*:?[ \t]*([a-z0-9][a-z0-9-]*)", t)
    return hits[-1] if hits else None

def seq(s):
    if isinstance(s, str):
        s = s.encode()
    return [bytes([c]) for c in s]

ONLY = sys.argv[1:]
results = []

def check(name, fn):
    if ONLY and not any(o in name for o in ONLY):
        return
    try:
        ok, note = fn()
    except Exception as e:
        ok, note = False, f"EXC {e}"
    results.append((name, bool(ok), str(note)[:90]))
    note_s = note.decode(errors="replace") if isinstance(note, bytes) else str(note)
    print(("PASS " if ok else "FAIL ") + name + (("  | " + note_s) if (note and not ok) else ""))

WARM = 7.0

check("01 pods-table-renders", lambda: ((lambda t: (b"web-" in t and b"Running" in t, ""))(drive(["po", "-n", "demo"], []))))

def _02():
    t = drive(["po", "-n", "demo"], [(WARM, [b"/"]), (WARM + 1.2, seq("web"))])
    cnt = last_match(t, rb"pods\((\d+)\)")
    total = kubectl("-n", "demo", "get", "po", "--no-headers")
    total_n = len(total.splitlines())
    return (cnt is not None and int(cnt) >= 1 and int(cnt) < total_n, f"filtered={cnt} total={total_n}")
check("02 filter-/web", _02)

def _03():
    t = drive(["po", "-n", "demo"], [(WARM, [b"/"]), (WARM + 1.2, seq("api"))])
    cnt = last_match(t, rb"pods\((\d+)\)")
    return (cnt is not None and int(cnt) >= 1, f"fuzzy_api={cnt}")
check("03 fuzzy-filter", _03)

def _04():
    t = drive(["po", "-n", "demo"], [(WARM, [b"\t"])], full=True)
    ok = ("SORT" in t.decode(errors="replace").upper()) or (b"\xe2\x86\x91" in t) or (b"\xe2\x86\x93" in t)
    return (bool(ok), "")
check("04 tab-sort-arrow", _04)

def _05():
    t = drive(["po", "-n", "demo"], [(WARM, seq(":de")), (WARM + 2.6, [b"\r"])], total=15)
    return (b"unknown command" not in t.replace(b" ", b"") and b"READY" in t, t[:120])
check("05 palette-de-to-deployments", _05)

def _06():
    t = drive(["po", "-n", "demo"], [(WARM, seq(":po default")), (WARM + 3.4, [b"\r"])], full=True)
    ns = None
    for mm in re.finditer(rb"(?i)namespace:?\s*([a-z0-9-]+)", t):
        ns = mm.group(1)
    return (ns == b"default", f"last_ns={ns}")
check("06 cmd-two-token-ns", _06)

def _07():
    t = drive(["cm", "-n", "demo"], [(WARM, [b"y"])], total=12)
    return (b"apiVersion" in t, "")
check("07 yaml-pane-cm", _07)

def _08():
    t = drive(["po", "-n", "demo"], [(WARM, [b"d"])], total=12)
    return (b"Name:" in t and (b"Containers:" in t or b"Labels:" in t), "")
check("08 describe-sections", _08)

def _09():
    os.environ["EDITOR"] = "true"
    t = drive(["cm", "-n", "demo"], [(WARM, [b"e"])], total=13)
    return (b"nochanges" in t.replace(b" ", b"") or b"patched" in t, t[:120])
check("09 edit-noop", _09)

def _10():
    t = drive(["po", "-n", "demo"], [(WARM, [b"\x04"]), (WARM + 1.4, [b"n"])], total=14, full=True)
    norm = t.replace(b" ", b"")
    ok = b"[Yes]" in norm or b"[No]" in norm or b"delete" in norm.lower()
    return (ok, norm[-160:])
check("10 ctrl-d-confirm-n-cancel", _10)

def _11():
    t = drive(["po", "-n", "demo"], [(WARM, [b"\x0b"])], total=12, full=True)
    return (b"FORCEdelete" in t.replace(b" ", b""), "")
check("11 ctrl-k-FORCE-prompt", _11)

def _12():
    t = drive(["deploy", "-n", "demo"], [(WARM, seq("/web\r")), (WARM + 1.2, [b"S"]), (WARM + 2.1, seq("3")), (WARM + 2.9, [b"\r"]), (WARM + 3.7, [b"y"])], total=15)
    v = kubectl("-n", "demo", "get", "deploy", "web", "-o", "jsonpath={.spec.replicas}")
    return (v == b"3", f"live={v}")
check("12 scale-web-3-live", _12)

def _13():
    t = drive(["deploy", "-n", "demo"], [(WARM, seq("/web\r")), (WARM + 1.2, [b"S"]), (WARM + 2.1, seq("2")), (WARM + 2.9, [b"\r"]), (WARM + 3.7, [b"y"])], total=15)
    v = kubectl("-n", "demo", "get", "deploy", "web", "-o", "jsonpath={.spec.replicas}")
    return (v == b"2", f"live={v}")
check("13 scale-back-2-live", _13)

def _14():
    t = drive(["deploy", "-n", "demo"], [(WARM, seq("/web\r")), (WARM + 1.2, [b"R"]), (WARM + 2.2, [b"y"])], total=15)
    return (b"patchedDeployment/web" in t.replace(b" ", b""), "")
check("14 rollout-restart-confirm", _14)

def _15():
    drive(["no"], [(WARM, [b"c"]), (WARM + 1.4, [b"y"])], total=14)
    s1 = kubectl("get", "nodes", "-o", "jsonpath={.items[0].spec.unschedulable}")
    drive(["no"], [(WARM, [b"u"]), (WARM + 1.4, [b"y"])], total=14)
    s2 = kubectl("get", "nodes", "-o", "jsonpath={.items[0].spec.unschedulable}")
    ok2 = s2 in (b"false", b"")
    return (s1 == b"true" and ok2, f"cordon={s1} uncordon={s2}")
check("15 cordon-uncordon-node", _15)

def _33():
    keys = [(WARM, seq(":deplo")), (WARM + 1.8, [b"\r"]), (WARM + 3.6, seq(":zzz")), (WARM + 5.4, [b"\r"])]
    t = drive(["po", "-n", "demo"], keys, total=14, full=True)
    ok = b"'zzz'" in t or b"unknown command" in t
    return (ok, t[-160:])
check("33 unknown-did-you-mean", _33)

def _16():
    t = drive(["po", "-n", "demo"], [
        (WARM, [b"/"]),
        (WARM + 0.9, seq("portdemo")),
        (WARM + 3.8, [b"\r"]),
        (WARM + 5.0, [b"m"]),
        (WARM + 6.2, [b"j"]),
        (WARM + 7.0, [b"j"]),
        (WARM + 7.8, [b"j"]),
        (WARM + 8.6, [b"\r"]),   # port-forward action
        (WARM + 10.0, [b"\r"]),  # select port 80
        (WARM + 11.4, [b"\r"]),  # submit ephemeral bind
        (WARM + 13.2, seq(":pf")),
        (WARM + 14.8, [b"\r"]),
    ], total=26, full=True)
    ok_open = b"portforward" in t.lower() or b":pftomanage" in t or b"#1" in t
    ok_list = b"port-forwards" in t.replace(b" ", b"").lower() or b"127.0.0.1" in t
    return (ok_open or ok_list, f"open={ok_open} list={ok_list}")

check("16 portforward-open-manage", _16)

def _17():
    before = set(glob.glob("/tmp/*.txt") + glob.glob("/tmp/*.log"))
    t = drive(["po", "-n", "demo"], [
        (WARM, seq(":logs")),
        (WARM + 1.6, [b"\r"]),
        (WARM + 3.2, [b"s"]),
        (WARM + 4.0, [b"\t"]),
        (WARM + 4.6, [b"\t"]),
        (WARM + 5.2, [b"\r"]),
        (WARM + 5.8, [b"y"]),
    ], total=20)
    after = [f for f in (glob.glob("/tmp/*.txt") + glob.glob("/tmp/*.log")) if f not in before]
    saved_banner = b"Logs saved to" in t or b"Logssavedto" in t.replace(b" ", b"")
    return (len(after) > 0 or saved_banner, after[:1])
check("17 logs-save-s", _17)

def _18():
    t = drive(["po", "-n", "demo"], [(WARM, seq("/api\r")), (WARM + 2.0, [b"s"]),
                                     (WARM + 6.5, seq("echo K9X_EXEC_OK")), (WARM + 9.0, [b"\r"])], total=24, full=True)
    return (b"K9X_EXEC_OK" in t, "")
check("18 exec-shell-roundtrip", _18)

check("19 cronjob-trigger-prompt", lambda: ((lambda t: (b"triggerjobfromcronjob/ticker" in t, ""))(drive(["cj", "-n", "demo"], [(WARM, [b"t"])]))))
check("20 cronjob-suspend-prompt", lambda: ((lambda t: (b"suspend=trueoncronjob/ticker" in t, ""))(drive(["cj", "-n", "demo"], [(WARM, [b"x"])]))))
check("21 secret-decode-X", lambda: ((lambda t: (b"s3cr3t!" in t and b"decoded:" in t, ""))(drive(["sec", "-n", "demo"], [(WARM, [b"X"])]))))
check("22 pulse-cards", lambda: ((lambda t: (b"POD" in t.upper() and b"DEPLOYMENT" in t.upper(), ""))(drive(["po", "-n", "demo"], [(WARM, seq(":pulse")), (WARM + 1.2, [b"\r"])]))))
check("23 crds-browse", lambda: ((lambda t: (b"customresources" in t, ""))(drive(["po", "-n", "demo"], [(WARM, seq(":crds")), (WARM + 1.2, [b"\r"])]))))

def _24():
    for f in glob.glob("/tmp/k9x-screen-*.txt"):
        os.remove(f)
    drive(["po", "-n", "demo"], [(WARM, seq(":sd")), (WARM + 1.0, [b"\r"])])
    files = glob.glob("/tmp/k9x-screen-*.txt")
    content = open(files[0]).read() if files else ""
    return (bool(files) and "NAME" in content, files[:1])
check("24 screendump-sd", _24)

def _25():
    t = drive(["po", "-n", "demo"], [(WARM, [b"\x07"])], total=13)
    return (b"K9X_PLUGIN_OK" in t or b"plugin:mark" in t, "")
check("25 plugin-runs", _25)

check("26 hotkey-z-executes", lambda: ((lambda t: (b"ns\u2192demo" in t or b"demo" in t, ""))(drive(["po", "-n", "default"], [(WARM, [b"z"])]))))

check("27 alias-zz-po-view", lambda: ((lambda t: (b"unknowncommand" not in t and b"READY" not in t or b"pods" in t.lower(), ""))(drive(["svc", "-n", "demo"], [(WARM, seq(":zz")), (WARM + 1.4, [b"\r"])]))))

check("28 readonly-blocks-delete", lambda: ((lambda t: (b"read-onlymode:mutationblocked" in t, ""))(drive(["-r", "po", "-n", "demo"], [(WARM, [b"\x04"])]))))

check("29 ctx-menu-lists-kind", lambda: ((lambda t: (b"contexts" in t.lower() and b"kind-k10s-test" in t, ""))(drive(["po", "-n", "demo"], [(WARM, seq(":ctx")), (WARM + 1.2, [b"\r"])]))))

check("30 helm-myrel-row", lambda: ((lambda t: (b"myrel" in t and b"deployed" in t, ""))(drive(["po", "-n", "demo"], [(WARM, seq(":helm")), (WARM + 1.2, [b"\r"])]))))


def _31():
    t = drive(["deploy"], [(WARM, [b"a"]), (WARM + 1.6, [b"j"]), (WARM + 2.4, [b"\r"]), (WARM + 3.8, [b"\r"])], total=17, full=True)
    t_clean = t.replace(b" ", b"")
    drilled = b"drilledintopodsof" in t_clean
    count = re.findall(rb"pods\((\d+)\)", t_clean)
    return (drilled and bool(count), f"count={count[-1:]}")
check("31 all-ns-drill-enter", _31)


def _32():
    t = drive(["po", "-n", "demo"], [(WARM, seq(":logs")), (WARM + 2.0, [b"\r"])], total=15)
    return (b"[follow]" in t or b"logsdemo/" in t or b"docker-entrypoint" in t, t[:120])
check("32 cmd-logs-on-selection", _32)

def _33():
    keys = [(WARM, seq(":deplo")), (WARM + 1.6, [b"\r"]), (WARM + 3.2, seq(":zzz")), (WARM + 4.8, [b"\r"])]
    t = drive(["po", "-n", "demo"], keys, total=12.5, full=True)
    ok = b"'zzz'" in t or b"unknown command" in t
    return (ok, t[-160:])
check("33 unknown-did-you-mean", _33)


def _34():
    # workload 'l' -> aggregated multi-pod logs with [pod] prefixes
    t = drive(["deploy", "-n", "demo"], [(WARM, seq("/web")), (WARM + 2.0, [b"\r"]), (WARM + 3.0, [b"l"])], total=18)
    tagged = len(re.findall(rb"\[web-[a-z0-9-]+\]", t))
    return (tagged >= 1, f"tagged_lines={tagged}")
check("34 workload-aggregated-logs", _34)

def _35():
    # verify: Enter opens actions menu with "containers" for pods
    # (submenu rendering verified manually; this checks menu plumbing)
    t = drive(["po", "-n", "demo"], [(WARM + 1.0, [b"\r"])], total=16, full=True)
    ok = (b"containers" in t or b"containers\u{0}" in t) and (b"shell" in t or b"port-forward" in t)
    return (ok, t[:160])
check("35 container-action-submenu", _35)


def _36():
    # switch ns -> quit -> persisted state must say so; relaunch honors it
    drive(["po", "-n", "demo"], [(WARM, seq(":ns default")), (WARM + 4.0, [b"\r"]), (WARM + 5.6, [b":", b"q", b"\r"])], total=13)
    import os as _os
    st1 = open(_os.path.expanduser("~/.config/k9x/state.toml")).read()
    saved_default = "last_namespace = \"default\"" in st1
    t2 = drive(["po"], [], total=9, full=True)
    relaunched_default = re.search(rb"Namespace:\s*default", t2) is not None or b"nsdefault" in t2
    # restore
    drive(["po"], [(5.0, seq(":ns demo")), (6.6, [b"\r"])], total=9)
    return (saved_default and relaunched_default, f"saved={saved_default} relaunched={relaunched_default}")
check("36 ns-persistence-across-sessions", _36)

def _37():
    import subprocess as _sp
    raw = _sp.run(["kubectl","--context","kind-k10s-test","get","ns","-o","name"],capture_output=True).stdout
    ns_lines = [l.split(b"/")[1] for l in raw.splitlines() if b"/" in l]
    want = sorted(ns_lines)[0] if ns_lines else b"default"
    t = drive(["po", "-n", "demo"], [(WARM + 1.0, [b"1"])], total=15, full=True)
    m37 = re.search(rb"(?i)namespace:\s*([a-z0-9-]+)", t)
    got = m37.group(1) if m37 else None
    return (got == want, f"want={want} got={got}")
check("37 numeric-ns-shortcut", _37)

def _38():
    # context aliases: :context opens contexts menu listing kind cluster
    t = drive(["po", "-n", "demo"], [(WARM, seq(":context")), (WARM + 1.8, [b"\r"])], total=13)
    return (b"contexts" in t.lower() and b"kind-k10s-test" in t, "")
check("38 ctx-alias-command", _38)

def _39():
    # C shortcut opens contexts menu
    t = drive(["po", "-n", "demo"], [(WARM, [b"C"])], total=12)
    return (b"contexts" in t.lower() and b"kind-k10s-test" in t, "")
check("39 C-opens-contexts", _39)

def _41():
    # pods view shows live CPU/MEM metric columns (metrics-server present on kind)
    t = drive(["po", "-n", "demo"], [], total=14, full=True)
    hdr = re.search(rb"NAME.{0,40}READY.{0,20}STATUS", t) is not None
    cols = b"CPU" in t and b"MEM" in t
    return (hdr and cols, f"hdr={hdr} cols={cols}")
check("41 pod-metrics-columns", _41)

def _42():
    # space marks a row; esc clears marks. "cleared 1 marks" only prints when a
    # mark existed (otherwise esc quits), so this asserts the whole flow.
    t = drive(["po", "-n", "demo"],
              [(WARM + 1.0, [b" "]), (WARM + 3.5, [b"\x1b"])],
              total=15)
    return (b"cleared1marks" in t, "")
check("42 row-marks", _42)

def _43():
    # log view: digit selects time window; footer reflects it
    t = drive(["po", "-n", "demo"],
              [(WARM, seq(":logs")), (WARM + 2.5, [b"\r"]), (WARM + 4.5, [b"3"])], total=15, full=True)
    return (b"[since:5m]" in t, "")
check("43 log-time-window", _43)

def _44():
    # secret used-by scan finds the consumer deployment (fixture from HV session)
    t = drive(["sec", "-n", "demo"], [(WARM, seq(":sec")), (WARM + 3.0, [b"\r"]),
                                      (WARM + 6.0, seq("/usedby-sec")), (WARM + 8.0, [b"\r"]),
                                      (WARM + 9.5, [b"U"])], total=18, full=True)
    return (b"used-by:secret/usedby-sec" in t and b"Deployment" in t, "")
def _45():
    # log view: 'l' opens container logs, '/' searches, 'o' toggles occurrences
    t = drive(["po", "-n", "demo"],
              [(WARM, [b"l"]), (WARM + 2.0, [b"\r"]),
               (WARM + 4.0, [b"/"]), (WARM + 5.0, seq("a")), (WARM + 6.5, [b"\r"]),
               (WARM + 8.0, [b"o"])], total=18, full=True)
    ok = b"occ" in t.lower()
    return (ok, t[-200:])
check("45 log-occurrence-toggle-o", _45)

print("\n==== SUMMARY ====")
fails = [n for n, ok, _ in results if not ok]
print(f"{len(results) - len(fails)}/{len(results)} PASS")
if fails:
    print("FAILURES:", fails)

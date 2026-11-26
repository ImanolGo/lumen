#!/usr/bin/env python3
"""Cold-start benchmark of Lumen and the GTK Lumen (lumen-gtk) on the same files.

For each run: start the app with the file, read its own timing logs until the
first view is on screen, then sample memory (RSS) and CPU time, wait 3 s idle,
sample CPU again, and close it. Runs open real windows; usage:

    python3 tools/bench.py [runs]            # median of `runs` (default 5)
    BENCH_JPEG=~/some/photo.jpg python3 tools/bench.py

Apps are taken from ~/.local/bin (`make install PREFIX=$HOME/.local`); an app
that is not installed is skipped. Results go to stdout as JSON, progress to stderr.
"""
import json, os, re, signal, statistics, subprocess, sys, time

APPS = {
    name: [path]
    for name, path in {
        "lumen-gtk": os.path.expanduser("~/.local/bin/lumen-gtk"),
        "lumen": os.path.expanduser("~/.local/bin/lumen"),
    }.items()
    if os.path.exists(path)
}
FILES = {
    name: path
    for name, path in {
        "PDF 42 p": "/usr/share/doc/ghostscript/GS9_Color_Management.pdf",
        "PDF 477 p": "/usr/share/doc/ghostscript/Ghostscript.pdf",
        "JPEG": os.path.expanduser(os.environ.get("BENCH_JPEG", "")),
        "PNG 5K": "/usr/share/wallpapers/cachyos-wallpapers/Cachy depths 5K.png",
    }.items()
    if path and os.path.isfile(path)
}
RUNS = int(sys.argv[1]) if len(sys.argv) > 1 else 5
ANSI = re.compile(r"\x1b\[[0-9;]*m")
TICK = os.sysconf("SC_CLK_TCK")


def cpu_ms(pid):
    with open(f"/proc/{pid}/stat") as f:
        parts = f.read().rsplit(")", 1)[1].split()
    return (int(parts[11]) + int(parts[12])) * 1000 / TICK


def rss_mb(pid):
    with open(f"/proc/{pid}/status") as f:
        for line in f:
            if line.startswith("VmRSS:"):
                return int(line.split()[1]) / 1024
    return 0.0


def num(line, key):
    m = re.search(key + r"=(?:Some\()?([0-9.]+)", line)
    return float(m.group(1)) if m else None


def run(app, path):
    env = dict(os.environ, RUST_LOG="info", LUMEN_THEME="dark")
    start = time.monotonic()
    p = subprocess.Popen(app + [path], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env)
    result = {}
    deadline = start + 20
    os.set_blocking(p.stdout.fileno(), False)
    buf = ""
    while time.monotonic() < deadline and "first_view" not in result:
        try:
            chunk = os.read(p.stdout.fileno(), 65536)
        except BlockingIOError:
            chunk = b""
        buf += chunk.decode("utf-8", "replace")
        for line in buf.splitlines():
            line = ANSI.sub("", line)
            if "window built" in line:
                result["window"] = num(line, "ms")
            if " opened" in line or "opened " in line:
                v = num(line, "load_ms")
                if v is not None:
                    result["load"] = v
            if "first view on screen" in line:
                result["first_view"] = num(line, "ms")
        time.sleep(0.01)
    if "first_view" in result:
        result["wall_first_view"] = (time.monotonic() - start) * 1000
        result["cpu_load"] = cpu_ms(p.pid)
        time.sleep(1.0)
        result["rss"] = rss_mb(p.pid)
        before = cpu_ms(p.pid)
        time.sleep(3.0)
        result["cpu_idle_3s"] = cpu_ms(p.pid) - before
        result["rss_settled"] = rss_mb(p.pid)
    p.send_signal(signal.SIGTERM)
    try:
        p.wait(timeout=5)
    except subprocess.TimeoutExpired:
        p.kill()
        p.wait()
    time.sleep(0.8)
    return result


def main():
    out = {}
    for fname, path in FILES.items():
        for _ in range(RUNS):
            for name, app in APPS.items():
                r = run(app, path)
                out.setdefault(fname, {}).setdefault(name, []).append(r)
                print(fname, name, r, file=sys.stderr, flush=True)
    summary = {}
    for fname, apps in out.items():
        for name, runs in apps.items():
            row = {}
            for key in ["window", "load", "first_view", "wall_first_view", "cpu_load", "rss", "rss_settled", "cpu_idle_3s"]:
                vals = [r[key] for r in runs if r.get(key) is not None]
                row[key] = round(statistics.median(vals), 1) if vals else None
            row["ok"] = sum(1 for r in runs if "first_view" in r)
            summary.setdefault(fname, {})[name] = row
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()

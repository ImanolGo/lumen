#!/usr/bin/env python3
"""Takes a picture of every scene of the interface, for looking at after a change.

    cargo build --release
    tools/screenshots/review.py OUTPUT_FOLDER [SCENE ...]

Each scene is taken in light and dark, at 1200x800 and at the smallest window (520x360), on an
invisible X display (Xvfb and ImageMagick are needed), so nothing on your desktop is touched. The
app is reached over LUMEN_CONTROL=stdio. A contact sheet per theme and size is made as well.
"""
import json, os, pathlib, shutil, subprocess, sys, tempfile, time

ROOT = pathlib.Path(__file__).resolve().parents[2]
APP = ROOT / "target/release/lumen-rs"
FIX = ROOT / "tests/fixtures"
PDF, FORM, PICTURE = FIX / "pages.pdf", FIX / "form.pdf", FIX / "pictures/g.png"

# scene: (file, what is shown on it)
SCENES = {
    "empty": (None, []),
    "picture": (PICTURE, []),
    "pdf": (PDF, []),
    "contents": (PDF, ["pages-tab"]),
    "menu": (PDF, ["menu"]),
    "menu-picture": (PICTURE, ["menu"]),
    "menu-view": (PDF, ["menu-view"]),
    "context": (PDF, ["context"]),
    "context-picture": (PICTURE, ["context"]),
    "open": (PDF, ["open-menu"]),
    "prefs": (PDF, ["prefs"]),
    "keys": (PDF, ["keys"]),
    "about": (PDF, ["about"]),
    "props": (PDF, ["props"]),
    "search": (PDF, ["search"]),
    "search-none": (PDF, ["search-none"]),
    "zoom-menu": (PICTURE, ["zoom-menu"]),
    "outline": (pathlib.Path("/usr/share/doc/ghostscript/Ghostscript.pdf"), ["contents-tab"]),
    "markup-search": (PDF, ["markup", "search"]),
    "loading": (PICTURE, ["loading"]),
    "goto": (PDF, ["goto"]),
    "palette": (PDF, ["palette"]),
    "export": (PDF, ["export"]),
    "markup": (PDF, ["markup"]),
    "colour": (PDF, ["markup", "colour"]),
    "form": (FORM, []),
    "sign": (PDF, ["sign"]),
    "pad": (PDF, ["pad"]),
    "edit": (PICTURE, ["edit"]),
    "region": (PICTURE, ["region"]),
    "discard": (PDF, ["discard"]),
    "crop": (PICTURE, ["edit", "crop"]),
    "resize": (PICTURE, ["edit", "resize"]),
    "adjust": (PICTURE, ["edit", "adjust"]),
    "organise": (PDF, ["organise"]),
    "chooser": (PDF, ["chooser"]),
    "print": (PDF, ["print"]),
    "quit": (PDF, ["quit"]),
    "overwrite": (PDF, ["overwrite"]),
    "trash": (PDF, ["trash"]),
    "locked": (FIX / "encrypted.pdf", []),
    "failed": (FIX / "gradient.avif", []),
    "toast": (PDF, ["toast"]),
    "error": (PDF, ["error"]),
    "slideshow": (PICTURE, ["slideshow"]),
}


class Lumen:
    def __init__(self, work, theme, size, file):
        (work / "state/lumen-rs").mkdir(parents=True, exist_ok=True)
        (work / "state/lumen-rs/state.toml").write_text(f"window = [{size[0]}, {size[1]}]\n")
        env = dict(os.environ, LUMEN_CONTROL="stdio", LUMEN_THEME=theme, LUMEN_NO_PORTAL="1", LUMEN_NO_REEXEC="1",
                   ICED_BACKEND=os.environ.get("ICED_BACKEND", "wgpu"), WGPU_POWER_PREF="low", LC_ALL="", LANGUAGE="en", LANG=os.environ.get("REVIEW_LANG", "en_US.UTF-8"))
        if "REVIEW_LANG" in os.environ:
            env["LANGUAGE"] = os.environ["REVIEW_LANG"].split("_")[0]
        for name in ("CONFIG", "DATA", "STATE", "CACHE"):
            env[f"XDG_{name}_HOME"] = str(work / name.lower())
        env.pop("WAYLAND_DISPLAY", None)
        args = [str(APP)] + ([str(file)] if file else [])
        self.app = subprocess.Popen(args, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                    stderr=subprocess.DEVNULL, text=True)

    def call(self, method, *args):
        self.app.stdin.write(json.dumps({"method": method, "args": args}) + "\n")
        self.app.stdin.flush()
        answer = json.loads(self.app.stdout.readline())
        if "error" in answer:
            raise SystemExit(f"{method}{args}: {answer['error']}")
        return answer["ok"]

    def settle(self, file):
        for _ in range(200):
            state = self.call("GetState")
            if not state["busy"] and (file is None or state["file"]):
                return
            time.sleep(0.05)

    def end(self):
        self.app.kill()
        self.app.wait()


def inside(out, scenes, size):
    """Runs under xvfb-run, with a screen of exactly the window's size."""
    for theme in ("light", "dark"):
        for scene in scenes:
            file, steps = SCENES[scene]
            with tempfile.TemporaryDirectory() as work:
                lumen = Lumen(pathlib.Path(work), theme, size, file)
                try:
                    lumen.settle(file)
                    for step in steps:
                        lumen.call("Show", step)
                        time.sleep(0.25)
                    lumen.settle(file)
                    time.sleep(0.6)
                    target = out / f"{scene}-{theme}-{size[0]}.png"
                    subprocess.run(["import", "-window", "root", str(target)], check=True)
                finally:
                    lumen.end()


def main():
    if sys.argv[1] == "--inside":
        out, w, h = pathlib.Path(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
        return inside(out, sys.argv[5:], (w, h))
    out = pathlib.Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    scenes = sys.argv[2:] or list(SCENES)
    for w, h in ((1200, 800), (520, 360)):
        subprocess.run(["xvfb-run", "-a", "-s", f"-screen 0 {w}x{h}x24", sys.executable, __file__, "--inside",
                        str(out), str(w), str(h), *scenes], check=True)
        for theme in ("light", "dark"):
            shots = [str(out / f"{s}-{theme}-{w}.png") for s in scenes]
            subprocess.run(["montage", "-label", "%t", *shots, "-tile", "4x", "-geometry", f"{min(w, 600)}x+6+6",
                            str(out / f"sheet-{theme}-{w}.png")], check=True)


if __name__ == "__main__":
    main()

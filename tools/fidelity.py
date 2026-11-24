#!/usr/bin/env python3
"""Compares how lumen-rs (hayro) and poppler draw the same PDF pages.

    python3 tools/fidelity.py OUT_DIR PDF_OR_FOLDER... [--per-file 6] [--per-producer 4] [--worst 12]

For each PDF it picks a few pages (first two, last, and some in between), draws them at 72 dpi
with `pdftoppm` and with lumen-rs (`cargo test --release render_corpus -- --ignored`), blurs both
a little (so anti-aliasing differences do not count) and measures how many pixels still differ.
It writes OUT_DIR/report.md (every page, worst first) and OUT_DIR/worst/*.png (poppler | lumen-rs |
difference) for the worst pages. Needs poppler-utils, numpy and Pillow.
"""
import argparse
import os
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

import numpy as np
from PIL import Image, ImageFilter

ROOT = Path(__file__).resolve().parent.parent
# A pixel counts as different when its blurred colour differs by more than this (of 255).
THRESHOLD = 40


def pdfinfo(path):
    try:
        out = subprocess.run(["pdfinfo", str(path)], capture_output=True, text=True, timeout=30).stdout
    except Exception:
        return None
    pages = re.search(r"^Pages:\s+(\d+)", out, re.M)
    producer = re.search(r"^Producer:\s+(.*)$", out, re.M)
    encrypted = re.search(r"^Encrypted:\s+yes", out, re.M)
    if not pages or encrypted:
        return None
    return int(pages.group(1)), (producer.group(1).strip() if producer else "unknown")


def pick_pages(count, per_file):
    wanted = {1, min(2, count), count}
    step = max(1, count // (per_file - 2)) if per_file > 3 else count
    wanted |= {p for p in range(1 + step, count, step)}
    return sorted(p for p in wanted if 1 <= p <= count)[:per_file]


def collect(paths, per_file, per_producer):
    files = []
    for p in paths:
        p = Path(p)
        files += sorted(p.rglob("*.pdf")) if p.is_dir() else [p]
    seen = defaultdict(int)
    chosen = []
    for f in files:
        info = pdfinfo(f)
        if not info:
            continue
        count, producer = info
        key = re.sub(r"[\d.]+", "", producer)[:40]
        if seen[key] >= per_producer:
            continue
        seen[key] += 1
        chosen.append((f, count, producer, pick_pages(count, per_file)))
    return chosen


def blur(img):
    return np.asarray(img.convert("RGB").filter(ImageFilter.GaussianBlur(1.0)), dtype=np.int16)


def compare(ref_path, got_path):
    ref, got = Image.open(ref_path).convert("RGB"), Image.open(got_path).convert("RGB")
    size_note = ""
    if ref.size != got.size:
        size_note = f"size {ref.size[0]}x{ref.size[1]} vs {got.size[0]}x{got.size[1]}"
        if abs(ref.size[0] - got.size[0]) > 2 or abs(ref.size[1] - got.size[1]) > 2:
            return 1.0, 255.0, size_note, ref, got, None
        w, h = min(ref.size[0], got.size[0]), min(ref.size[1], got.size[1])
        ref, got = ref.crop((0, 0, w, h)), got.crop((0, 0, w, h))
    a, b = blur(ref), blur(got)
    diff = np.abs(a - b).max(axis=2)
    return float((diff > THRESHOLD).mean()), float(diff.mean()), size_note, ref, got, diff


def montage(ref, got, diff, out_path, label):
    w, h = ref.size
    heat = Image.new("RGB", (w, h), (255, 255, 255))
    if diff is not None:
        d = np.clip(diff * 3, 0, 255).astype("uint8")
        heat = Image.fromarray(np.stack([np.full_like(d, 255), 255 - d, 255 - d], axis=2))
    got = got.resize((w, h)) if got.size != (w, h) else got
    sheet = Image.new("RGB", (w * 3 + 20, h + 18), (90, 90, 90))
    for i, im in enumerate((ref, got, heat)):
        sheet.paste(im, (i * (w + 10), 18))
    from PIL import ImageDraw
    draw = ImageDraw.Draw(sheet)
    for i, name in enumerate(("poppler", "lumen-rs", "difference")):
        draw.text((i * (w + 10) + 4, 3), f"{name}   {label}" if i == 0 else name, fill=(255, 255, 255))
    scale = min(1.0, 1800 / sheet.width)
    if scale < 1.0:
        sheet = sheet.resize((int(sheet.width * scale), int(sheet.height * scale)))
    sheet.save(out_path)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("pdfs", nargs="+")
    ap.add_argument("--per-file", type=int, default=6)
    ap.add_argument("--per-producer", type=int, default=4)
    ap.add_argument("--worst", type=int, default=12)
    ap.add_argument("--dpi", type=int, default=72, help="resolution to compare at (96 is a 100%% zoom on screen)")
    args = ap.parse_args()
    out = Path(args.out).resolve()
    (out / "ref").mkdir(parents=True, exist_ok=True)
    (out / "got").mkdir(exist_ok=True)
    (out / "worst").mkdir(exist_ok=True)

    chosen = collect(args.pdfs, args.per_file, args.per_producer)
    print(f"{len(chosen)} PDFs, {sum(len(c[3]) for c in chosen)} pages", file=sys.stderr)
    (out / "list.txt").write_text("".join(f"{f}|{','.join(map(str, pages))}\n" for f, _, _, pages in chosen))

    for n, (f, _, _, pages) in enumerate(chosen):
        for p in pages:
            subprocess.run(["pdftoppm", "-r", str(args.dpi), "-f", str(p), "-l", str(p), "-singlefile", "-png", str(f), str(out / "ref" / f"{n}-{p}")],
                           capture_output=True, timeout=120)

    env = dict(os.environ, LUMEN_CORPUS_LIST=str(out / "list.txt"), LUMEN_CORPUS_OUT=str(out / "got"),
               LUMEN_CORPUS_SCALE=str(args.dpi / 72))
    run = subprocess.run(["cargo", "test", "--release", "--locked", "render_corpus", "--", "--ignored", "--nocapture"],
                         cwd=ROOT, env=env, capture_output=True, text=True)
    lines = [l for l in run.stdout.splitlines() if l.startswith(("OK\t", "OPEN-ERR\t", "RENDER-ERR\t"))]
    if not lines:
        print(run.stdout[-2000:], run.stderr[-2000:], file=sys.stderr)
        sys.exit("lumen-rs rendered nothing")

    rows, errors, times = [], [], []
    for line in lines:
        parts = line.split("\t")
        if parts[0] != "OK":
            errors.append(parts)
            continue
        n, p, ms = int(parts[1]), int(parts[2]), int(parts[3])
        ref, got = out / "ref" / f"{n}-{p}.png", out / "got" / f"{n}-{p}.png"
        if not ref.exists():
            continue
        frac, mean, note, ri, gi, diff = compare(ref, got)
        rows.append(dict(n=n, page=p, frac=frac, mean=mean, note=note, file=chosen[n][0], producer=chosen[n][2], ms=ms, ri=ri, gi=gi, diff=diff))
        times.append(ms)
    rows.sort(key=lambda r: -r["frac"])

    for i, r in enumerate(rows[: args.worst]):
        name = f"{i + 1:02d}-{Path(r['file']).stem[:30]}-p{r['page']}.png".replace(" ", "_")
        montage(r["ri"], r["gi"], r["diff"], out / "worst" / name, f"{r['frac'] * 100:.1f}% differ")
        r["montage"] = name

    fr = np.array([r["frac"] for r in rows])
    lines = ["# Rendering fidelity: lumen-rs (hayro) against poppler", "",
             f"{len(chosen)} PDFs, {len(rows)} pages compared at {args.dpi} dpi. A pixel counts as different when, after a 1 px blur, "
             f"its colour differs by more than {THRESHOLD}/255.", "",
             f"- median page: **{np.median(fr) * 100:.2f}%** of pixels differ; mean {fr.mean() * 100:.2f}%",
             f"- pages over 1%: {(fr > 0.01).sum()}; over 5%: {(fr > 0.05).sum()}; over 20%: {(fr > 0.20).sum()}",
             f"- render time (one page): median {int(np.median(times))} ms, slowest {max(times)} ms",
             f"- pages that failed to open or render in lumen-rs: {len(errors)}", ""]
    if errors:
        lines += ["## Failures", ""] + [f"- `{e[-2] if len(e) > 3 else ''}` {e[0]}: {e[-1]}" for e in errors] + [""]
    lines += ["## Pages, worst first", "", "| % differ | mean diff | file | page | producer | note |", "|---|---|---|---|---|---|"]
    for r in rows:
        lines.append(f"| {r['frac'] * 100:.2f} | {r['mean']:.1f} | {Path(r['file']).name} | {r['page']} | {r['producer'][:40]} | {r['note']} |")
    (out / "report.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines[:12]))
    print(f"\nreport: {out / 'report.md'}\nworst:  {out / 'worst'}")


if __name__ == "__main__":
    main()

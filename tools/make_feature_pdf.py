#!/usr/bin/env python3
"""Makes a PDF that exercises drawing features (gradients, transparency, clipping, CMYK, dashes,
text modes, embedded fonts, images with alpha, a rotated page) for tools/fidelity.py.

    python3 tools/make_feature_pdf.py OUTPUT.pdf
"""
import io
import math
import sys
from pathlib import Path

from PIL import Image, ImageDraw
from reportlab.lib import colors
from reportlab.lib.colors import CMYKColor, Color
from reportlab.lib.pagesizes import A4
from reportlab.lib.utils import ImageReader
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.pdfgen import canvas

W, H = A4


def find_font(*names):
    for base in ("/usr/share/fonts", "/usr/local/share/fonts", str(Path.home() / ".local/share/fonts")):
        for n in names:
            for p in Path(base).rglob(n):
                return p
    return None


def alpha_image():
    img = Image.new("RGBA", (200, 200), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    for r in range(100, 0, -4):
        d.ellipse([100 - r, 100 - r, 100 + r, 100 + r], fill=(255 - r * 2, 60 + r, 200, int(255 * (r / 100))))
    return img


def page1(c):
    c.setFont("Helvetica-Bold", 16)
    c.drawString(40, H - 40, "Features 1: paint")
    # linear and radial gradients
    c.saveState()
    p = c.beginPath(); p.rect(40, H - 160, 150, 90); c.clipPath(p, stroke=0, fill=0)
    c.linearGradient(40, H - 160, 190, H - 70, (colors.HexColor("#1f3a5f"), colors.HexColor("#f2b134")))
    c.restoreState()
    c.saveState()
    p = c.beginPath(); p.rect(210, H - 160, 150, 90); c.clipPath(p, stroke=0, fill=0)
    c.radialGradient(285, H - 115, 70, (colors.white, colors.HexColor("#c0392b"), colors.black), (0, 0.5, 1))
    c.restoreState()
    # transparency: overlapping circles
    for i, col in enumerate((colors.red, colors.green, colors.blue)):
        c.setFillColor(col); c.setFillAlpha(0.5)
        c.circle(440 + i * 22, H - 115 + (i % 2) * 14, 38, stroke=0, fill=1)
    c.setFillAlpha(1)
    # CMYK swatches
    for i, (cc, m, y, k) in enumerate([(1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1), (1, 1, 0, 0), (0.3, 0.7, 0.1, 0.2)]):
        c.setFillColor(CMYKColor(cc, m, y, k)); c.rect(40 + i * 56, H - 215, 50, 40, stroke=0, fill=1)
    # dashes, caps, joins, widths
    c.setStrokeColor(colors.black)
    for i, (w, dash) in enumerate([(0.25, None), (0.5, None), (1, None), (2, (6, 3)), (4, (1, 6)), (6, (12, 4, 2, 4))]):
        c.setLineWidth(w)
        c.setDash(list(dash)) if dash else c.setDash()
        c.line(40, H - 240 - i * 14, 300, H - 240 - i * 14)
    c.setDash()
    for i, (cap, join) in enumerate([(0, 0), (1, 1), (2, 2)]):
        c.setLineCap(cap); c.setLineJoin(join); c.setLineWidth(14); c.setStrokeColor(colors.HexColor("#2f5d8a"))
        p = c.beginPath(); p.moveTo(330 + i * 80, H - 330); p.lineTo(355 + i * 80, H - 250); p.lineTo(380 + i * 80, H - 330)
        c.drawPath(p, stroke=1, fill=0)
    c.setLineCap(0); c.setLineJoin(0)
    # clipping with a star and a picture
    c.saveState()
    p = c.beginPath()
    pts = [(120 + 70 * math.cos(math.pi / 2 + k * 4 * math.pi / 5), H - 440 + 70 * math.sin(math.pi / 2 + k * 4 * math.pi / 5)) for k in range(5)]
    p.moveTo(*pts[0]); [p.lineTo(*q) for q in pts[1:]]; p.close()
    c.clipPath(p, stroke=0, fill=0, fillMode=0)
    c.linearGradient(50, H - 510, 190, H - 370, (colors.HexColor("#27ae60"), colors.HexColor("#8e44ad")))
    c.restoreState()
    # image with alpha over a pattern
    c.setFillColor(colors.HexColor("#f0e6d2")); c.rect(240, H - 500, 130, 130, stroke=0, fill=1)
    c.drawImage(ImageReader(alpha_image()), 240, H - 500, 130, 130, mask="auto")
    # blend through a soft overlap of text and shapes
    c.setFillColor(colors.HexColor("#d35400")); c.setFillAlpha(0.35)
    c.setFont("Helvetica-Bold", 60); c.drawString(400, H - 440, "Aa")
    c.setFillAlpha(1)
    # text render modes
    c.setFillColor(colors.HexColor("#1f3a5f")); c.setStrokeColor(colors.HexColor("#c0392b")); c.setLineWidth(1)
    t = c.beginText(40, H - 560); t.setFont("Times-Bold", 34); t.setTextRenderMode(2); t.textLine("Fill and stroke"); c.drawText(t)
    t = c.beginText(40, H - 600); t.setFont("Helvetica", 34); t.setTextRenderMode(1); t.textLine("Outline only"); c.drawText(t)
    # standard fonts
    y = H - 640
    for name in ("Helvetica", "Times-Roman", "Courier", "Symbol", "ZapfDingbats", "Helvetica-Oblique", "Times-Italic", "Courier-Bold"):
        c.setFillColor(colors.black); c.setFont(name, 11)
        c.drawString(40, y, f"{name}: The quick brown fox jumps over the lazy dog 0123456789 abc"); y -= 14
    # hairlines like the ones that differed in the corpus
    c.setLineWidth(0.1); c.setStrokeColor(colors.black)
    for k in range(40):
        c.line(380 + k * 3, H - 560, 380 + k * 3 + 30, H - 700)


def page2(c):
    c.setFont("Helvetica-Bold", 16); c.drawString(40, H - 40, "Features 2: text")
    ttf = find_font("DejaVuSans.ttf")
    serif = find_font("DejaVuSerif.ttf")
    y = H - 80
    if ttf:
        pdfmetrics.registerFont(TTFont("DejaVu", str(ttf)))
        c.setFont("DejaVu", 14)
        for s in ("Embedded TrueType: ÀÉÎÕÜ àéîõü ñÑ ß çÇ", "Greek: αβγδε ΑΒΓΔΕ  Cyrillic: Привет мир", "Symbols: ← → ↑ ↓ ∑ ∫ ≠ ≤ ≥ √ ∞ © ® ™ € £ ¥"):
            c.drawString(40, y, s); y -= 22
    if serif:
        pdfmetrics.registerFont(TTFont("DejaVuSerif", str(serif)))
        c.setFont("DejaVuSerif", 14); c.drawString(40, y, "Serif: Typography is the craft of arranging type."); y -= 28
    cjk = find_font("NotoSansCJK-Regular.ttc", "NotoSansCJKjp-Regular.otf")
    if cjk:
        try:
            pdfmetrics.registerFont(TTFont("CJK", str(cjk), subfontIndex=0) if cjk.suffix == ".ttc" else TTFont("CJK", str(cjk)))
        except Exception as e:  # a font reportlab cannot embed: skip the line
            print("no CJK line:", e)
            cjk = None
    if cjk:
        c.setFont("CJK", 16); c.drawString(40, y, "日本語のテキスト 中文文本 한국어"); y -= 28
    # sizes and rotation
    for size in (6, 8, 10, 12, 16, 24):
        c.setFont("Helvetica", size); c.drawString(40, y, f"{size} pt: Sphinx of black quartz, judge my vow"); y -= size + 6
    c.saveState(); c.translate(430, H - 220); c.rotate(30); c.setFont("Times-Italic", 22); c.drawString(0, 0, "Rotated 30°"); c.restoreState()
    c.saveState(); c.translate(430, H - 300); c.scale(1.6, 0.7); c.setFont("Helvetica-Bold", 22); c.drawString(0, 0, "Squashed"); c.restoreState()
    # a table of thin rules
    c.setLineWidth(0.5); y0 = H - 560
    for r in range(8):
        c.line(40, y0 - r * 20, 500, y0 - r * 20)
    for cx in range(5):
        c.line(40 + cx * 115, y0, 40 + cx * 115, y0 - 140)
    c.setFont("Helvetica", 10)
    for r in range(7):
        for cx in range(4):
            c.drawString(46 + cx * 115, y0 - 14 - r * 20, f"cell {r + 1},{cx + 1}")


def page3(c):
    c.setPageRotation(90)
    c.setFont("Helvetica-Bold", 20); c.drawString(60, H - 60, "Features 3: this page is rotated 90°")
    c.setFillColor(colors.HexColor("#2f5d8a")); c.rect(60, H - 200, 200, 100, stroke=0, fill=1)
    c.setFillColor(colors.white); c.setFont("Helvetica", 14); c.drawString(70, H - 150, "top-left of the unrotated page")


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "features.pdf"
    c = canvas.Canvas(out, pagesize=A4, pageCompression=1)
    c.setTitle("Lumen rendering features")
    for fn in (page1, page2, page3):
        fn(c); c.showPage()
    c.save()
    print("wrote", out)


if __name__ == "__main__":
    main()

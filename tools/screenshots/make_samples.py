#!/usr/bin/env python3
"""Makes the sample pictures and PDFs used for Lumen's screenshots.

Everything is drawn here (nothing is a photo or someone else's document), so the screenshots
can be published. Needs Pillow, numpy and reportlab.

    python3 tools/screenshots/make_samples.py OUTPUT_DIR
"""
import math
import random
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter
from reportlab.lib import colors
from reportlab.lib.pagesizes import A4
from reportlab.lib.styles import ParagraphStyle, getSampleStyleSheet
from reportlab.lib.units import mm
from reportlab.pdfbase.pdfmetrics import stringWidth
from reportlab.pdfgen import canvas
from reportlab.platypus import (BaseDocTemplate, CondPageBreak, Frame, Image as RLImage, PageBreak, PageTemplate,
                                Paragraph, Spacer, Table, TableStyle)

W, H = 2400, 1600

PALETTES = {
    # name: (sky top, sky bottom, sun, mountains far -> near, water)
    "dawn": ((38, 52, 108), (255, 170, 120), (255, 232, 190), [(120, 100, 150), (80, 70, 120), (42, 38, 78)], (70, 90, 150)),
    "golden-hour": ((52, 96, 160), (255, 196, 110), (255, 244, 210), [(150, 120, 110), (100, 82, 84), (54, 44, 56)], (200, 140, 90)),
    "forest-lake": ((70, 130, 170), (200, 225, 215), (255, 255, 235), [(110, 150, 140), (60, 110, 100), (28, 70, 64)], (60, 120, 120)),
    "dusk": ((24, 24, 70), (230, 110, 120), (255, 190, 150), [(110, 70, 120), (66, 44, 92), (30, 24, 56)], (90, 60, 120)),
    "arctic": ((120, 170, 215), (230, 240, 250), (255, 255, 255), [(190, 205, 225), (150, 170, 200), (96, 120, 160)], (140, 175, 205)),
    "ember": ((30, 20, 40), (210, 80, 50), (255, 170, 90), [(110, 40, 50), (70, 26, 40), (34, 16, 28)], (120, 50, 60)),
}


def ridge(rng, base, amp, rough):
    """A mountain skyline: smoothed random heights across the width."""
    pts = np.zeros(W)
    step = 1.0
    for octave in range(6):
        n = max(2, int(W / (300 / (2 ** octave))))
        xs = np.linspace(0, W, n)
        ys = np.array([rng.uniform(-1, 1) for _ in range(n)]) * (rough ** octave)
        pts += np.interp(np.arange(W), xs, ys)
        step *= 0.5
    pts = (pts - pts.mean()) / (np.abs(pts).max() + 1e-6)
    return base + pts * amp


def landscape(name, seed):
    top, bottom, sun, mountains, water = PALETTES[name]
    rng = random.Random(seed)
    horizon = int(H * 0.62)
    img = np.zeros((H, W, 3), dtype=np.float32)
    # sky
    t = np.linspace(0, 1, horizon)[:, None, None] ** 0.8
    img[:horizon] = np.array(top) * (1 - t) + np.array(bottom) * t
    # sun glow
    sx, sy = rng.randint(int(W * 0.25), int(W * 0.75)), int(horizon * 0.82)
    yy, xx = np.mgrid[0:horizon, 0:W]
    d = np.sqrt((xx - sx) ** 2 + (yy - sy) ** 2)
    glow = np.exp(-(d / 360.0) ** 2)[:, :, None]
    img[:horizon] = img[:horizon] * (1 - glow * 0.75) + np.array(sun) * glow * 0.75
    disc = (d < 70)[:, :, None]
    img[:horizon] = np.where(disc, np.array(sun), img[:horizon])
    # mountains
    for i, col in enumerate(mountains):
        sky = ridge(rng, horizon - 330 + i * 120, 150 - i * 25, 0.62)
        for x in range(W):
            y0 = int(sky[x])
            fade = 0.0 if i == 0 else 0.0
            img[y0:horizon, x] = col
    # water: reflect the scene above the horizon
    water_h = H - horizon
    refl = img[horizon - water_h:horizon][::-1].copy() if horizon - water_h >= 0 else np.flip(img[:horizon], 0)[:water_h]
    refl = refl[:water_h]
    ripple = (np.sin(np.linspace(0, 80, water_h))[:, None, None] * 3.0)
    wcol = np.array(water, dtype=np.float32)
    img[horizon:horizon + refl.shape[0]] = refl * 0.55 + wcol * 0.45 + ripple
    pil = Image.fromarray(np.clip(img, 0, 255).astype("uint8"))
    # water streaks along the glow
    draw = ImageDraw.Draw(pil, "RGBA")
    for _ in range(260):
        y = rng.randint(horizon + 8, H - 5)
        x = int(sx + rng.gauss(0, 200 * (y - horizon) / (H - horizon) + 20))
        length = rng.randint(30, 180)
        draw.line([(x - length, y), (x + length, y)], fill=(*sun, rng.randint(20, 70)), width=rng.randint(1, 3))
    # a few birds
    for _ in range(5):
        bx, by = rng.randint(200, W - 200), rng.randint(120, horizon - 420)
        s = rng.randint(14, 30)
        draw.arc([bx - s, by - s // 2, bx, by + s // 2], 200, 340, fill=(30, 30, 40, 220), width=3)
        draw.arc([bx, by - s // 2, bx + s, by + s // 2], 200, 340, fill=(30, 30, 40, 220), width=3)
    pil = pil.filter(ImageFilter.GaussianBlur(0.8))
    # film grain
    grain = np.random.default_rng(seed).normal(0, 3.0, (H, W, 1))
    arr = np.clip(np.asarray(pil, dtype=np.float32) + grain, 0, 255).astype("uint8")
    return Image.fromarray(arr)


def pictures(out):
    for i, name in enumerate(PALETTES):
        landscape(name, seed=11 + i * 7).save(out / f"{name}.jpg", quality=90)
    return [out / f"{n}.jpg" for n in PALETTES]


# ------------------------------------------------------------------ PDFs

def field_guide(out, picture):
    """A report with a cover, outline, headings, a table, a picture and links."""
    path = out / "Field guide.pdf"
    styles = getSampleStyleSheet()
    body = ParagraphStyle("body", parent=styles["BodyText"], fontName="Helvetica", fontSize=10.5, leading=15, spaceAfter=8)
    h1 = ParagraphStyle("h1", parent=styles["Heading1"], fontName="Helvetica-Bold", fontSize=22, spaceBefore=6, spaceAfter=10,
                        textColor=colors.HexColor("#1f3a5f"))
    h2 = ParagraphStyle("h2", parent=styles["Heading2"], fontName="Helvetica-Bold", fontSize=14, spaceBefore=10, spaceAfter=6,
                        textColor=colors.HexColor("#2f5d8a"))
    cap = ParagraphStyle("cap", parent=body, fontSize=8.5, textColor=colors.HexColor("#666666"), alignment=1)

    class Doc(BaseDocTemplate):
        def afterFlowable(self, flowable):
            if isinstance(flowable, Paragraph) and flowable.style.name in ("h1", "h2"):
                text = flowable.getPlainText()
                key = f"b{abs(hash(text))}"
                self.canv.bookmarkPage(key)
                self.canv.addOutlineEntry(text, key, level=0 if flowable.style.name == "h1" else 1)

    def footer(c, d):
        c.saveState()
        c.setFont("Helvetica", 8)
        c.setFillColor(colors.HexColor("#888888"))
        c.drawString(20 * mm, 12 * mm, "A field guide to the lakeshore")
        c.drawRightString(A4[0] - 20 * mm, 12 * mm, f"Page {d.page}")
        c.restoreState()

    doc = Doc(str(path), pagesize=A4, leftMargin=22 * mm, rightMargin=22 * mm, topMargin=22 * mm, bottomMargin=22 * mm,
              title="A field guide to the lakeshore", author="Lumen sample")
    frame = Frame(doc.leftMargin, doc.bottomMargin, doc.width, doc.height, id="f")
    doc.addPageTemplates([PageTemplate(id="p", frames=[frame], onPage=footer)])

    text = [
        "The shore of a lake changes with the hour. At dawn the water is a mirror, the hills behind it are blue "
        "shapes, and the only movement is a line of birds crossing the light.",
        "This guide follows one stretch of shoreline through a day. Each chapter pairs what you can see with "
        "when and where to look, so a short walk is enough to find most of it.",
        "Bring warm clothes even in summer: the air over cold water is always a few degrees colder than the "
        "path behind you, and the best light comes before the sun has warmed anything.",
    ]
    flow = [Spacer(1, 70 * mm), Paragraph("A field guide to the lakeshore", ParagraphStyle(
        "title", parent=h1, fontSize=34, leading=40, alignment=0)),
        Paragraph("Dawn to dusk on one stretch of water", ParagraphStyle("sub", parent=body, fontSize=15, leading=20,
                                                                      textColor=colors.HexColor("#555555"))),
        Spacer(1, 20 * mm), RLImage(str(picture), width=doc.width, height=doc.width * H / W), PageBreak()]
    chapters = [
        ("Reading the water", ["Still water first.", "Wind ruffles."]),
        ("Birds you will meet", ["Herons stand.", "Geese commute."]),
        ("Light and weather", ["The hour matters.", "Clouds change everything."]),
        ("Walking the shore", ["The path loops.", "Benches are marked."]),
        ("Keeping notes", ["Write the time.", "Sketch the sky."]),
    ]
    openings = [
        "Still water first. On a calm morning the surface holds the whole sky, and the line where the far shore meets "
        "its reflection is the sharpest edge you will see all day. Wind arrives as a darker band crossing the mirror.",
        "Herons stand in the shallows by the jetty from first light, waiting; geese commute across open water in loose "
        "lines; kingfishers flash between the overhanging branches in the morning, too fast to follow but easy to hear.",
        "The hour matters more than the season. Low sun turns the hills from blue to rose to gold in a quarter of "
        "an hour, and thin cloud at the horizon can double the length of the show.",
        "The path loops the lake in about two hours. Benches are placed where the view opens, and each has a number "
        "so you can say exactly where you saw something.",
        "Write down the time before anything else. A sketch of the sky, even a bad one, will tell you more a year "
        "from now than a long description, and a list of the day's sightings turns a walk into a record.",
    ]
    for n, (title, _) in enumerate(chapters, 1):
        flow.append(CondPageBreak(90 * mm))
        flow.append(Paragraph(f"{n}. {title}", h1))
        flow.append(Paragraph(openings[n - 1], body))
        for t in text[1:]:
            flow.append(Paragraph(t, body))
        flow.append(Paragraph("What to look for", h2))
        flow.append(Paragraph('See also <a href="https://example.org/lakeshore" color="#2f5d8a">the walkers\' website</a>.', body))
        flow.append(Paragraph("Look low across the surface first, then lift your eyes to the far shore. "
                              "Small changes show up at the edges: where the reeds meet the water, where the "
                              "light leaves the hills, where a ripple starts for no reason you can see.", body))
        if n == 2:
            data = [["Bird", "Where", "Best hour"], ["Grey heron", "Shallows by the jetty", "Dawn"],
                    ["Canada goose", "Open water, mid-lake", "All day"], ["Kingfisher", "Overhanging branches", "Morning"],
                    ["Great crested grebe", "Reed edge", "Spring dusk"]]
            tbl = Table(data, colWidths=[doc.width * 0.34, doc.width * 0.42, doc.width * 0.24])
            tbl.setStyle(TableStyle([("BACKGROUND", (0, 0), (-1, 0), colors.HexColor("#2f5d8a")),
                                     ("TEXTCOLOR", (0, 0), (-1, 0), colors.white),
                                     ("FONTNAME", (0, 0), (-1, 0), "Helvetica-Bold"), ("FONTSIZE", (0, 0), (-1, -1), 10),
                                     ("ROWBACKGROUNDS", (0, 1), (-1, -1), [colors.white, colors.HexColor("#eef3f8")]),
                                     ("GRID", (0, 0), (-1, -1), 0.4, colors.HexColor("#c9d4e0")),
                                     ("TOPPADDING", (0, 0), (-1, -1), 6), ("BOTTOMPADDING", (0, 0), (-1, -1), 6)]))
            flow += [Spacer(1, 6), tbl, Spacer(1, 10)]
        if n == 3:
            flow += [Spacer(1, 6), RLImage(str(picture), width=doc.width * 0.8, height=doc.width * 0.8 * H / W),
                     Paragraph("Low light over the water, a little before sunrise.", cap), Spacer(1, 8)]
    doc.build(flow)
    return path


def membership_form(out):
    """A one-page fillable form with text fields, check boxes, radio buttons, a list and a signature box."""
    path = out / "Membership form.pdf"
    c = canvas.Canvas(str(path), pagesize=A4, pageCompression=0)
    c.setTitle("Lakeshore Walkers: membership form")
    w, h = A4
    c.setFillColor(colors.HexColor("#1f3a5f"))
    c.rect(0, h - 34 * mm, w, 34 * mm, stroke=0, fill=1)
    c.setFillColor(colors.white)
    c.setFont("Helvetica-Bold", 24)
    c.drawString(22 * mm, h - 20 * mm, "Lakeshore Walkers")
    c.setFont("Helvetica", 12)
    c.drawString(22 * mm, h - 28 * mm, "Membership form")
    c.setFillColor(colors.HexColor("#333333"))
    form = c.acroForm
    x, y = 22 * mm, h - 52 * mm
    label = lambda t, yy: (c.setFont("Helvetica-Bold", 10), c.drawString(x, yy, t))
    field = dict(borderStyle="solid", borderWidth=0.6, borderColor=colors.HexColor("#9fb2c8"),
                 fillColor=colors.HexColor("#eaf1fb"), textColor=colors.black, fontName="Helvetica", fontSize=11)
    label("Full name", y)
    form.textfield(name="name", x=x, y=y - 10 * mm, width=w - 44 * mm, height=7.5 * mm, **field)
    y -= 24 * mm
    label("Email", y)
    form.textfield(name="email", x=x, y=y - 10 * mm, width=(w - 44 * mm) * 0.58, height=7.5 * mm, **field)
    c.setFont("Helvetica-Bold", 10)
    c.drawString(x + (w - 44 * mm) * 0.62, y, "Phone")
    form.textfield(name="phone", x=x + (w - 44 * mm) * 0.62, y=y - 10 * mm, width=(w - 44 * mm) * 0.38, height=7.5 * mm, **field)
    y -= 24 * mm
    label("Membership", y)
    for i, (value, text) in enumerate([("individual", "Individual"), ("family", "Family"), ("student", "Student")]):
        form.radio(name="kind", tooltip=text, value=value, selected=False, x=x + i * 42 * mm, y=y - 9 * mm,
                   buttonStyle="circle", borderStyle="solid", borderWidth=0.8, borderColor=colors.HexColor("#6b87a8"),
                   fillColor=colors.HexColor("#eaf1fb"), size=11)
        c.setFont("Helvetica", 10.5)
        c.drawString(x + i * 42 * mm + 7 * mm, y - 7.5 * mm, text)
    y -= 24 * mm
    label("Walks you would like to join", y)
    for i, (name, text) in enumerate([("dawn", "Dawn birdwatch"), ("evening", "Evening loop"), ("weekend", "Weekend long walk"),
                                      ("photo", "Photography morning")]):
        yy = y - 9 * mm - (i // 2) * 9 * mm
        xx = x + (i % 2) * 70 * mm
        form.checkbox(name=name, tooltip=text, x=xx, y=yy, size=11, buttonStyle="check", borderWidth=0.8,
                      borderColor=colors.HexColor("#6b87a8"), fillColor=colors.HexColor("#eaf1fb"), checked=False)
        c.setFont("Helvetica", 10.5)
        c.drawString(xx + 7 * mm, yy + 1.5 * mm, text)
    y -= 36 * mm
    label("How did you hear about us?", y)
    form.choice(name="heard", value="Friend", options=["Friend", "Newspaper", "Notice board", "Online", "Other"],
                x=x, y=y - 10 * mm, width=70 * mm, height=7.5 * mm, fieldFlags="combo", **field)
    y -= 26 * mm
    label("Notes", y)
    form.textfield(name="notes", x=x, y=y - 24 * mm, width=w - 44 * mm, height=20 * mm, fieldFlags="multiline", **field)
    y -= 40 * mm
    c.setFont("Helvetica", 8.5)
    c.setFillColor(colors.HexColor("#666666"))
    c.drawString(x, y, "By signing you agree to follow the walk leaders' instructions and to leave no trace.")
    c.setFillColor(colors.HexColor("#333333"))
    c.setFont("Helvetica-Bold", 10)
    c.drawString(x, y - 9 * mm, "Signature")
    c.setStrokeColor(colors.HexColor("#6b87a8"))
    c.setLineWidth(0.8)
    c.line(x, y - 24 * mm, x + 90 * mm, y - 24 * mm)
    c.drawString(x + 110 * mm, y - 9 * mm, "Date")
    form.textfield(name="date", x=x + 110 * mm, y=y - 18 * mm, width=45 * mm, height=7.5 * mm, **field)
    c.showPage()
    c.save()
    return path


def trip_notes(out, pictures_list):
    """Twelve pages, each with a different picture and colour, so page thumbnails are easy to tell apart."""
    path = out / "Trip notes.pdf"
    c = canvas.Canvas(str(path), pagesize=A4)
    c.setTitle("Trip notes")
    w, h = A4
    days = ["Arrival", "The jetty", "Up the east path", "Rain", "Market day", "The long walk", "Boat to the island",
            "Quiet morning", "Hills at noon", "Swimming", "Last evening", "Going home"]
    tints = ["#e8f0fb", "#fbeee8", "#e9f6ec", "#f5ecfa", "#fff6dc", "#e6f4f6"]
    for i, day in enumerate(days):
        c.setFillColor(colors.HexColor(tints[i % len(tints)]))
        c.rect(0, 0, w, h, stroke=0, fill=1)
        c.setFillColor(colors.HexColor("#1f3a5f"))
        c.setFont("Helvetica-Bold", 30)
        c.drawString(22 * mm, h - 32 * mm, f"Day {i + 1}")
        c.setFont("Helvetica", 15)
        c.setFillColor(colors.HexColor("#555555"))
        c.drawString(22 * mm, h - 42 * mm, day)
        pic = pictures_list[i % len(pictures_list)]
        c.drawImage(str(pic), 22 * mm, h - 150 * mm, width=w - 44 * mm, height=(w - 44 * mm) * H / W, preserveAspectRatio=True)
        c.setFillColor(colors.HexColor("#333333"))
        c.setFont("Helvetica", 11)
        y = h - 175 * mm
        for line in ["Notes for the day.", "Weather, the path, what we saw, and what to bring next time."]:
            c.drawString(22 * mm, y, line)
            y -= 6 * mm
        c.setFont("Helvetica", 8)
        c.setFillColor(colors.HexColor("#999999"))
        c.drawRightString(w - 22 * mm, 12 * mm, f"{i + 1}")
        c.showPage()
    c.save()
    return path


def main():
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "samples")
    out.mkdir(parents=True, exist_ok=True)
    pics = pictures(out)
    field_guide(out, pics[1])
    membership_form(out)
    trip_notes(out, pics)
    print("made", *sorted(p.name for p in out.iterdir()), sep="\n  ")


if __name__ == "__main__":
    main()

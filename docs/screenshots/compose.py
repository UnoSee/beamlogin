"""Put phone screenshots into realistic phone frames with captions, and build the hero image.

The frame is a generic modern phone, not any real model. It has a metal rim with side buttons, a
thin glass bezel, a punch-hole camera, a status bar, a browser address bar and a home indicator.
Shapes are drawn at 3x and scaled down, so the edges are smooth.
"""
import os
from PIL import Image, ImageDraw, ImageFilter, ImageFont

OUT = "/work/out"
FINAL = "/work/final"
os.makedirs(FINAL, exist_ok=True)
SANS = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"
SANS_B = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"

SS = 3                                   # supersampling factor for smooth edges
SCREEN_W, SCREEN_H = 780, 1688           # a 390 x 844 pt screen at 2x
STATUS_H, BAR_H = 96, 92                 # status bar and browser address bar
BEZEL, RIM = 20, 8                       # glass border and metal rim
SCREEN_R = 104                           # screen corner radius
MARGIN = 56                              # room around the body for buttons and shadow
PAGE_ADDR = "192.168.0.10:8765"         # matches IP in capture.py


def aa_layer(size, draw):
    """Run draw(ImageDraw, scale) on a 3x canvas and scale it down for anti-aliased shapes."""
    big = Image.new("RGBA", (size[0] * SS, size[1] * SS), (0, 0, 0, 0))
    draw(ImageDraw.Draw(big), SS)
    return big.resize(size, Image.LANCZOS)


def aa_mask(size, radius):
    big = Image.new("L", (size[0] * SS, size[1] * SS), 0)
    ImageDraw.Draw(big).rounded_rectangle([0, 0, size[0] * SS - 1, size[1] * SS - 1], radius=radius * SS, fill=255)
    return big.resize(size, Image.LANCZOS)


def shade(rgb, amount):
    return tuple(max(0, min(255, c + amount)) for c in rgb)


def screen(name):
    """The phone's screen: status bar, address bar, then the page screenshot."""
    shot = Image.open(f"{OUT}/{name}.png").convert("RGB")
    bg = shot.getpixel((5, 5))
    dark = sum(bg) < 300
    fg = (242, 241, 236) if dark else (22, 21, 19)
    mute = (150, 148, 142) if dark else (112, 110, 104)
    scr = Image.new("RGBA", (SCREEN_W, SCREEN_H), bg + (255,))
    scr.paste(shot.crop((0, 0, SCREEN_W, SCREEN_H - STATUS_H - BAR_H)), (0, STATUS_H + BAR_H))

    def chrome(d, s):
        def P(*v):
            return [x * s for x in v]

        cy = STATUS_H / 2 + 4
        # time
        d.text((60 * s, cy * s), "9:41", font=ImageFont.truetype(SANS_B, 31 * s), fill=fg, anchor="lm")
        # battery
        x1 = SCREEN_W - 58
        x0 = x1 - 52
        d.rounded_rectangle(P(x0, cy - 13, x1, cy + 13), radius=8 * s, outline=mute, width=3 * s)
        d.rounded_rectangle(P(x0 + 5, cy - 8, x0 + 5 + 34, cy + 8), radius=4 * s, fill=fg)
        d.rounded_rectangle(P(x1 + 3, cy - 5, x1 + 7, cy + 5), radius=2 * s, fill=mute)
        # wi-fi: two arcs and a dot, fanning up from one point
        wx, wy = x0 - 34, cy + 12
        for r in (27, 17):
            d.arc(P(wx - r, wy - r, wx + r, wy + r), 225, 315, fill=fg, width=5 * s)
        d.pieslice(P(wx - 8, wy - 8, wx + 8, wy + 8), 225, 315, fill=fg)
        # signal bars
        sx = wx - 40 - 4 * 11
        for i, h in enumerate((8, 13, 18, 24)):
            d.rounded_rectangle(P(sx + i * 11, cy + 12 - h, sx + i * 11 + 7, cy + 12), radius=2 * s, fill=fg)
        # punch-hole camera
        cx = SCREEN_W / 2
        d.ellipse(P(cx - 17, cy - 17, cx + 17, cy + 17), fill=(6, 6, 8))
        d.ellipse(P(cx - 6, cy - 9, cx + 2, cy - 1), fill=(34, 36, 52))
        # browser address bar
        top, bot = STATUS_H + 8, STATUS_H + BAR_H - 14
        d.rounded_rectangle(P(26, top, SCREEN_W - 26, bot), radius=(bot - top) / 2 * s,
                            fill=shade(bg, 16 if dark else -14))
        my = (top + bot) / 2
        d.ellipse(P(52, my - 12, 76, my + 12), outline=mute, width=3 * s)          # site info icon
        d.line(P(64, my - 2, 64, my + 6), fill=mute, width=3 * s)
        d.ellipse(P(62, my - 8, 66, my - 4), fill=mute)
        d.text((SCREEN_W / 2 * s, my * s), PAGE_ADDR, font=ImageFont.truetype(SANS, 28 * s), fill=fg, anchor="mm")
        for k in (-10, 0, 10):                                                       # menu dots
            d.ellipse(P(SCREEN_W - 62, my + k - 3, SCREEN_W - 56, my + k + 3), fill=mute)
        # home indicator
        d.rounded_rectangle(P(SCREEN_W / 2 - 134, SCREEN_H - 26, SCREEN_W / 2 + 134, SCREEN_H - 16),
                            radius=5 * s, fill=fg)

    scr.alpha_composite(aa_layer(scr.size, chrome))
    return scr


def phone(name):
    """A screenshot inside a phone body, with a soft shadow."""
    scr = screen(name)
    bw, bh = SCREEN_W + 2 * (BEZEL + RIM), SCREEN_H + 2 * (BEZEL + RIM)
    br = SCREEN_R + BEZEL + RIM
    W, H = bw + 2 * MARGIN, bh + 2 * MARGIN
    M = MARGIN
    canvas = Image.new("RGBA", (W, H), (0, 0, 0, 0))

    shadow = Image.new("L", (W, H), 0)
    ImageDraw.Draw(shadow).rounded_rectangle([M + 10, M + 26, M + bw - 10, M + bh + 14], radius=br, fill=120)
    canvas.paste(Image.new("RGBA", (W, H), (0, 0, 0, 255)), (0, 0), shadow.filter(ImageFilter.GaussianBlur(24)))

    def body(d, s):
        def P(*v):
            return [x * s for x in v]

        button = (58, 58, 64)
        d.rounded_rectangle(P(M - 6, M + 300, M + 6, M + 400), radius=4 * s, fill=button)      # volume up
        d.rounded_rectangle(P(M - 6, M + 430, M + 6, M + 530), radius=4 * s, fill=button)      # volume down
        d.rounded_rectangle(P(M + bw - 6, M + 360, M + bw + 6, M + 520), radius=4 * s, fill=button)  # power
        d.rounded_rectangle(P(M, M, M + bw, M + bh), radius=br * s, fill=(86, 86, 94))         # metal rim edge
        d.rounded_rectangle(P(M + 2, M + 2, M + bw - 2, M + bh - 2), radius=(br - 2) * s, fill=(44, 44, 50))
        d.rounded_rectangle(P(M + RIM, M + RIM, M + bw - RIM, M + bh - RIM), radius=(br - RIM) * s,
                            fill=(7, 7, 9))                                                  # glass bezel

    canvas.alpha_composite(aa_layer((W, H), body))
    off = M + RIM + BEZEL
    canvas.paste(scr, (off, off), aa_mask(scr.size, SCREEN_R))
    return canvas


def row(items, out):
    """items: [(screenshot name, caption)] -> one image of phones with captions."""
    frames = [phone(n) for n, _ in items]
    cap_font = ImageFont.truetype(SANS_B, 36)
    gap, cap_h = 24, 96
    W = sum(f.width for f in frames) + gap * (len(frames) - 1)
    H = max(f.height for f in frames) + cap_h
    img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    x = 0
    for f, (_, cap) in zip(frames, items):
        img.paste(f, (x, 0), f)
        tw = d.textlength(cap, font=cap_font)
        d.text((x + (f.width - tw) / 2, f.height + 10), cap, font=cap_font, fill=(128, 128, 136, 255))
        x += f.width + gap
    img = img.resize((W // 2, H // 2), Image.LANCZOS)
    img.save(f"{FINAL}/{out}.png", optimize=True)
    print("wrote", out, img.size)


row([("p-login", "1  Open the page"), ("p-ready", "2  Paste the code"), ("p-done", "3  Signed in")], "phone-login-flow")
row([("p-token", "Paste a setup token"), ("p-token-done", "Token accepted"), ("p-failed", "When a code fails")],
    "phone-token-and-errors")
row([("p-login-dark", "Dark mode"), ("p-ready-dark", "Ready to send"), ("p-done-dark", "Signed in")], "phone-dark-mode")

# hero: the terminal with the phone beside it, overlapping its right edge
term = Image.open(f"{OUT}/terminal-login.png").convert("RGBA")
ph = phone("p-ready")
scale = (term.height * 1.22) / ph.height
ph = ph.resize((int(ph.width * scale), int(ph.height * scale)), Image.LANCZOS)
W = term.width + int(ph.width * 0.78)
H = max(term.height + 140, ph.height)
hero = Image.new("RGBA", (W, H), (0, 0, 0, 0))
hero.paste(term, (0, (H - term.height) // 2 + 40), term)
hero.alpha_composite(ph, (W - ph.width, 0))
hero = hero.resize((W // 2, H // 2), Image.LANCZOS)
hero.save(f"{FINAL}/hero.png", optimize=True)
print("wrote hero", hero.size)

# terminal screens: halve to a sensible README size
for name in ("terminal-login", "terminal-signed-in", "terminal-direct-link", "terminal-token", "terminal-token-works",
             "terminal-doctor", "terminal-install", "terminal-logout"):
    t = Image.open(f"{OUT}/{name}.png")
    t = t.resize((t.width // 2, t.height // 2), Image.LANCZOS)
    t.save(f"{FINAL}/{name}.png", optimize=True)
    print("wrote", name, t.size)

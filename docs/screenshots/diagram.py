"""Draw the "How it works" diagram: five steps in a line, left to right, and render light and
dark PNGs with Chromium.

Run in the beamlogin-shots image (see the README). Writes final/how-it-works-light.png and
final/how-it-works-dark.png.
"""
import html
import os
import subprocess

FINAL = "/work/final"
os.makedirs(FINAL, exist_ok=True)

THEMES = {
    "light": dict(fg="#1a1915", mute="#6f6d66", card="#ffffff", line="#e2ded3", arrow="#b9b4a7",
                  machine="#c2572f", machine_soft="#f8e9e2", phone="#3d6fb6", phone_soft="#e6eef9"),
    "dark": dict(fg="#f2f0ea", mute="#a19e95", card="#211f1c", line="#3a3732", arrow="#6b675f",
                 machine="#e07a52", machine_soft="#3a261d", phone="#6f9fe0", phone_soft="#1d2a3d"),
}

# 24x24 stroked icons
ICONS = {
    "terminal": '<rect x="2.5" y="4" width="19" height="16" rx="3"/><path d="M7 9.5l3 2.5-3 2.5M12.5 15h4.5"/>',
    "qr": '<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/>'
          '<rect x="3" y="14" width="7" height="7" rx="1.5"/><path d="M14 14h3v3M21 14v.01M14 21h.01M17.5 21H21v-3.5"/>',
    "signin": '<circle cx="12" cy="8" r="4"/><path d="M4.5 20.5c1.2-3.8 4-5.5 7.5-5.5s6.3 1.7 7.5 5.5"/>',
    "send": '<path d="M21 3L10 14M21 3l-7 18-4-7-7-4 18-7z"/>',
    "check": '<circle cx="12" cy="12" r="9"/><path d="M7.5 12.5l3 3 6-6.5"/>',
}

# (where, icon, title, explanation lines)
STEPS = [
    ("machine", "terminal", "Run beamlogin", ["A QR code appears", "in the terminal"]),
    ("phone", "qr", "Scan the QR code", ["Opens a one-time page", "on your phone"]),
    ("phone", "signin", "Sign in to Claude", ["claude.ai gives you", "a one-time code"]),
    ("phone", "send", "Send to terminal", ["Paste the code and tap", "the button"]),
    ("machine", "check", "You're signed in", ["Claude Code starts", "on the machine"]),
]
WHERE = {"machine": "On the machine", "phone": "On your phone"}

CARD_W, CARD_H, GAP, PAD = 210, 276, 52, 16
W = PAD * 2 + CARD_W * len(STEPS) + GAP * (len(STEPS) - 1)
H = CARD_H + 96


def svg(t):
    e = html.escape
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" '
           f'font-family="DejaVu Sans, sans-serif">']
    a = out.append
    top = 12
    for i, (where, icon, title, lines) in enumerate(STEPS):
        x = PAD + i * (CARD_W + GAP)
        cx = x + CARD_W / 2
        color, soft = t[where], t[where + "_soft"]
        a(f'<rect x="{x}" y="{top}" width="{CARD_W}" height="{CARD_H}" rx="20" fill="{t["card"]}" '
          f'stroke="{t["line"]}" stroke-width="1.5"/>')
        # where it happens
        tag = WHERE[where]
        tw = len(tag) * 8.2 + 26
        a(f'<rect x="{cx - tw / 2}" y="{top + 18}" width="{tw}" height="28" rx="14" fill="{soft}"/>')
        a(f'<text x="{cx}" y="{top + 37}" font-size="14" font-weight="bold" text-anchor="middle" '
          f'fill="{color}">{e(tag)}</text>')
        # icon with step number
        iy = top + 112
        a(f'<circle cx="{cx}" cy="{iy}" r="40" fill="{soft}"/>')
        a(f'<g transform="translate({cx - 18} {iy - 18}) scale(1.5)" fill="none" stroke="{color}" '
          f'stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">{ICONS[icon]}</g>')
        a(f'<circle cx="{cx + 30}" cy="{iy - 30}" r="15" fill="{color}"/>')
        a(f'<text x="{cx + 30}" y="{iy - 25}" font-size="15" font-weight="bold" text-anchor="middle" '
          f'fill="{t["card"]}">{i + 1}</text>')
        # title and explanation
        a(f'<text x="{cx}" y="{top + 200}" font-size="19" font-weight="bold" text-anchor="middle" '
          f'fill="{t["fg"]}">{e(title)}</text>')
        for k, line in enumerate(lines):
            a(f'<text x="{cx}" y="{top + 232 + k * 23}" font-size="15.5" text-anchor="middle" '
              f'fill="{t["mute"]}">{e(line)}</text>')
        # arrow to the next step
        if i < len(STEPS) - 1:
            ax0, ax1, ay = x + CARD_W + 10, x + CARD_W + GAP - 10, top + CARD_H / 2
            a(f'<path d="M{ax0} {ay}H{ax1 - 2}M{ax1 - 11} {ay - 9}L{ax1} {ay}L{ax1 - 11} {ay + 9}" fill="none" '
              f'stroke="{t["arrow"]}" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/>')
    note = ("The code travels only over your local network. "
            "Your password stays between your phone and claude.ai.")
    a(f'<text x="{W / 2}" y="{top + CARD_H + 52}" font-size="16" text-anchor="middle" fill="{t["mute"]}">'
      f'{e(note)}</text>')
    a('</svg>')
    return "\n".join(out)


for name, theme in THEMES.items():
    s = svg(theme)
    page = f"/work/diagram-{name}.html"
    with open(page, "w", encoding="utf-8") as f:
        f.write(f'<!doctype html><meta charset="utf-8"><style>html,body{{margin:0;background:transparent}}'
                f'svg{{display:block}}</style>{s}')
    out = f"{FINAL}/how-it-works-{name}.png"
    subprocess.run(["chromium", "--headless=new", "--no-sandbox", "--disable-gpu", "--hide-scrollbars",
                    f"--window-size={W},{H}", "--force-device-scale-factor=2", "--default-background-color=00000000",
                    f"--screenshot={out}", f"file://{page}"],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=90)
    print("wrote", out)

"""Generates assets/icon/AppIcon.svg (1024×1024, macOS icon grid): an orange broom on graphite.

Render it into the app's icon files with ./render_icon.sh.
"""
import math, pathlib

S, BODY = 1024, 824                      # canvas and icon body, per Apple's macOS grid
O = (S - BODY) / 2                        # 100 px margin leaves room for the drop shadow


def squircle(x0, y0, size, n=5.0, steps=720):
    """Superellipse |x|^n + |y|^n = 1: the continuous-corner shape of macOS icons."""
    a = size / 2
    cx, cy = x0 + a, y0 + a
    pts = []
    for i in range(steps):
        t = 2 * math.pi * i / steps
        c, s = math.cos(t), math.sin(t)
        x = cx + a * math.copysign(abs(c) ** (2 / n), c)
        y = cy + a * math.copysign(abs(s) ** (2 / n), s)
        pts.append(f"{x:.1f},{y:.1f}")
    return "M" + " L".join(pts) + " Z"


body = squircle(O, O, BODY)

svg = f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {S} {S}" width="{S}" height="{S}">
<defs>
  <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0" stop-color="#ECEEF2"/><stop offset="1" stop-color="#C9CED7"/>
  </linearGradient>
  <linearGradient id="handle" x1="0" y1="0" x2="1" y2="0">
    <stop offset="0" stop-color="#6B7280"/><stop offset="1" stop-color="#4B5160"/>
  </linearGradient>
  <linearGradient id="bristles" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0" stop-color="#FFB547"/><stop offset="1" stop-color="#F08A1C"/>
  </linearGradient>
  <filter id="shadow" x="-20%" y="-20%" width="140%" height="140%">
    <feGaussianBlur in="SourceAlpha" stdDeviation="12"/><feOffset dy="12" result="b"/>
    <feFlood flood-color="#000" flood-opacity="0.28"/><feComposite in2="b" operator="in"/>
    <feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge>
  </filter>
  <filter id="lift" x="-30%" y="-30%" width="160%" height="160%">
    <feGaussianBlur in="SourceAlpha" stdDeviation="10"/><feOffset dy="10" result="b"/>
    <feFlood flood-color="#000" flood-opacity="0.16"/><feComposite in2="b" operator="in"/>
    <feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge>
  </filter>
</defs>
<path d="{body}" fill="url(#bg)" filter="url(#shadow)"/>
<path d="{body}" fill="none" stroke="#000" stroke-opacity="0.06" stroke-width="3"/>
<g filter="url(#lift)" transform="translate(-10 -16) rotate(-32 512 540)">
  <rect x="492" y="160" width="40" height="390" rx="20" fill="url(#handle)"/>
  <rect x="436" y="528" width="152" height="60" rx="16" fill="#4B5160"/>
  <rect x="436" y="551" width="152" height="11" fill="#2E3340"/>
  <path d="M442 586 H582 L644 826 Q512 852 380 826 Z" fill="url(#bristles)"/>
  <g stroke="#B8620D" stroke-width="8" stroke-linecap="round" opacity="0.5">
    <path d="M468 604 L424 812"/><path d="M497 604 L479 826"/><path d="M527 604 L545 826"/><path d="M556 604 L600 812"/>
  </g>
</g>
<g fill="#9AA1AE">
  <circle cx="742" cy="772" r="17"/><circle cx="800" cy="712" r="11"/><circle cx="808" cy="804" r="9"/><circle cx="760" cy="676" r="7"/>
</g>
</svg>
'''
out = pathlib.Path(__file__).with_name("AppIcon.svg")
out.write_text(svg)
print("wrote", out)

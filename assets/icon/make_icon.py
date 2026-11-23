"""Generates assets/icon/AppIcon.svg (1024×1024, macOS icon grid)."""
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
        pts.append(f"{x:.2f},{y:.2f}")
    return "M" + " L".join(pts) + " Z"

def sparkle(cx, cy, r, k=0.16):
    """Four-point star with concave sides."""
    q = r * k
    return (f"M{cx},{cy-r} Q{cx+q},{cy-q} {cx+r},{cy} Q{cx+q},{cy+q} {cx},{cy+r} "
            f"Q{cx-q},{cy+q} {cx-r},{cy} Q{cx-q},{cy-q} {cx},{cy-r} Z")

def arc(cx, cy, r, a0, a1):
    """Clockwise arc from angle a0 to a1 (degrees, 0 = 3 o'clock, y down)."""
    p = lambda a: (cx + r * math.cos(math.radians(a)), cy + r * math.sin(math.radians(a)))
    (x0, y0), (x1, y1) = p(a0), p(a1)
    large = 1 if (a1 - a0) % 360 > 180 else 0
    return f"M{x0:.2f},{y0:.2f} A{r},{r} 0 {large} 1 {x1:.2f},{y1:.2f}", (x1, y1)

body = squircle(O, O, BODY)
cx, cy = 512, 524
ring, (ex, ey) = arc(cx, cy, 236, 112, 408)   # 296° sweep, gap at lower right

svg = f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {S} {S}" width="{S}" height="{S}">
  <defs>
    <linearGradient id="bg" x1="0.15" y1="0" x2="0.85" y2="1">
      <stop offset="0" stop-color="#3FE6CF"/>
      <stop offset="0.45" stop-color="#1C9DF2"/>
      <stop offset="1" stop-color="#3A47E0"/>
    </linearGradient>
    <linearGradient id="gloss" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.32"/>
      <stop offset="0.5" stop-color="#FFFFFF" stop-opacity="0"/>
    </linearGradient>
    <radialGradient id="glow" cx="{cx}" cy="{cy}" r="300" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.38"/>
      <stop offset="1" stop-color="#FFFFFF" stop-opacity="0"/>
    </radialGradient>
    <linearGradient id="sweep" x1="{cx-236}" y1="{cy+236}" x2="{cx+236}" y2="{cy-120}" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.18"/>
      <stop offset="0.6" stop-color="#FFFFFF" stop-opacity="0.75"/>
      <stop offset="1" stop-color="#FFFFFF" stop-opacity="1"/>
    </linearGradient>
    <linearGradient id="star" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#FFFFFF"/>
      <stop offset="1" stop-color="#E3F1FF"/>
    </linearGradient>
    <filter id="shadow" x="-20%" y="-20%" width="140%" height="140%">
      <feGaussianBlur in="SourceAlpha" stdDeviation="14"/>
      <feOffset dy="14" result="b"/>
      <feFlood flood-color="#000000" flood-opacity="0.30"/>
      <feComposite in2="b" operator="in"/>
      <feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge>
    </filter>
    <filter id="soft" x="-30%" y="-30%" width="160%" height="160%">
      <feGaussianBlur in="SourceAlpha" stdDeviation="10"/>
      <feOffset dy="8" result="b"/>
      <feFlood flood-color="#0B2A8A" flood-opacity="0.35"/>
      <feComposite in2="b" operator="in"/>
      <feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge>
    </filter>
    <clipPath id="clip"><path d="{body}"/></clipPath>
  </defs>

  <path d="{body}" fill="url(#bg)" filter="url(#shadow)"/>
  <g clip-path="url(#clip)">
    <rect width="{S}" height="{S}" fill="url(#gloss)"/>
    <circle cx="{cx}" cy="{cy}" r="300" fill="url(#glow)"/>
  </g>
  <path d="{body}" fill="none" stroke="#FFFFFF" stroke-opacity="0.22" stroke-width="3"/>

  <g filter="url(#soft)">
    <path d="{ring}" fill="none" stroke="url(#sweep)" stroke-width="58" stroke-linecap="round"/>
    <path d="{sparkle(cx, cy, 142)}" fill="url(#star)"/>
    <path d="{sparkle(ex + 6, ey - 6, 58)}" fill="#FFFFFF"/>
    <path d="{sparkle(cx - 232, cy - 226, 34)}" fill="#FFFFFF" fill-opacity="0.95"/>
  </g>
</svg>
'''
out = pathlib.Path(__file__).with_name("AppIcon.svg")
out.write_text(svg)
print("wrote", out)

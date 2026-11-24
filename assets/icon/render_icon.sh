#!/bin/sh
# Renders assets/icon/AppIcon.svg into assets/AppIcon.icns, assets/AppIcon.png and
# assets/icon/AppIcon-256.png (shown in the About window). macOS only.
#
# Quick Look draws the SVG with its blur shadows but always on an opaque background, so the
# icon is drawn once on white and once on black and the transparency is recovered from the
# difference: alpha = 1 - (white - black).
set -e
cd "$(dirname "$0")"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

DATA=$(base64 -i AppIcon.svg)
for c in white black; do
  printf '<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024"><rect width="1024" height="1024" fill="%s"/><image width="1024" height="1024" href="data:image/svg+xml;base64,%s"/></svg>' \
    "$c" "$DATA" > "$WORK/$c.svg"
done
qlmanage -t -s 1024 -o "$WORK" "$WORK/white.svg" "$WORK/black.svg" >/dev/null 2>&1

cat > "$WORK/matte.swift" <<'SWIFT'
import AppKit
let a = CommandLine.arguments
func load(_ p: String) -> NSBitmapImageRep { NSBitmapImageRep(data: try! Data(contentsOf: URL(fileURLWithPath: p)))! }
let w = load(a[1]), b = load(a[2]), n = w.pixelsWide
let out = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: n, pixelsHigh: n, bitsPerSample: 8, samplesPerPixel: 4,
                           hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: n * 4, bitsPerPixel: 32)!
let o = out.bitmapData!
var p = [Int](repeating: 0, count: 4), q = [Int](repeating: 0, count: 4)
for y in 0..<n { for x in 0..<n {
  w.getPixel(&p, atX: x, y: y); b.getPixel(&q, atX: x, y: y)
  let alpha = max(0, min(255, 255 - (p[0] - q[0] + p[1] - q[1] + p[2] - q[2]) / 3))
  let i = (y * n + x) * 4
  for c in 0..<3 { o[i + c] = UInt8(alpha == 0 ? 0 : min(255, q[c] * 255 / alpha)) }
  o[i + 3] = UInt8(alpha)
}}
try! out.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: a[3]))
SWIFT
swiftc -O "$WORK/matte.swift" -o "$WORK/matte"
"$WORK/matte" "$WORK/white.svg.png" "$WORK/black.svg.png" "$WORK/icon.png"

mkdir "$WORK/AppIcon.iconset"
for s in 16 32 128 256 512; do
  sips -z $s $s "$WORK/icon.png" --out "$WORK/AppIcon.iconset/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) "$WORK/icon.png" --out "$WORK/AppIcon.iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$WORK/AppIcon.iconset" -o ../AppIcon.icns
cp "$WORK/icon.png" ../AppIcon.png
sips -z 256 256 "$WORK/icon.png" --out AppIcon-256.png >/dev/null
echo "wrote assets/AppIcon.icns, assets/AppIcon.png, assets/icon/AppIcon-256.png"

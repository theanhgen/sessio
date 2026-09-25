// Draw sessio's icon, logo and social card into docs/brand/, from one description of the icon.
//
// The icon is the dashboard in miniature: the selected session row (plum) with its running dot
// and the `s` of sessio, between two quieter rows. The letters are Fraunces SemiBold, turned into
// outlines here, so the SVGs and PNGs look the same on a machine without the font.
//
//   swift scripts/brand.swift            # needs network once, for the font
//
// Everything it writes is committed; run it again only to change the mark. docs/DESIGN.md
// ("Brand") has the geometry and the reasons for it.
import AppKit
import CoreText
import Foundation

let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
let out = root.appendingPathComponent("docs/brand")
try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)

// ---------- the font ----------

let fontURL = FileManager.default.temporaryDirectory.appendingPathComponent("sessio-Fraunces-600.ttf")
if !FileManager.default.fileExists(atPath: fontURL.path) {
    let css = try String(contentsOf: URL(string: "https://fonts.googleapis.com/css2?family=Fraunces:wght@600")!, encoding: .utf8)
    guard let r = css.range(of: #"https://[^)]+\.ttf"#, options: .regularExpression) else { fatalError("no ttf in Google Fonts css") }
    try Data(contentsOf: URL(string: String(css[r]))!).write(to: fontURL)
}
let desc = (CTFontManagerCreateFontDescriptorsFromURL(fontURL as CFURL) as! [CTFontDescriptor])[0]
func fraunces(_ size: CGFloat) -> CTFont { CTFontCreateWithFontDescriptor(desc, size, nil) }

/// `text` as one path, origin at the left end of its baseline, y down (SVG's way round).
func textPath(_ text: String, size: CGFloat) -> (path: CGPath, width: CGFloat, ink: CGRect) {
    let font = fraunces(size)
    let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: [.init(kCTFontAttributeName as String): font]))
    let path = CGMutablePath()
    for run in CTLineGetGlyphRuns(line) as! [CTRun] {
        let n = CTRunGetGlyphCount(run)
        var glyphs = [CGGlyph](repeating: 0, count: n), pos = [CGPoint](repeating: .zero, count: n)
        CTRunGetGlyphs(run, CFRange(), &glyphs); CTRunGetPositions(run, CFRange(), &pos)
        for i in 0..<n {
            guard let g = CTFontCreatePathForGlyph(font, glyphs[i], nil) else { continue }
            path.addPath(g, transform: CGAffineTransform(a: 1, b: 0, c: 0, d: -1, tx: pos[i].x, ty: 0))
        }
    }
    return (path, CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil)), path.boundingBoxOfPath)
}

func svgPath(_ p: CGPath) -> String {
    var d = ""
    let f = { (v: CGFloat) in String(format: "%.2f", v).replacingOccurrences(of: ".00", with: "") }
    p.applyWithBlock { e in
        let pt = e.pointee.points
        switch e.pointee.type {
        case .moveToPoint: d += "M\(f(pt[0].x)) \(f(pt[0].y))"
        case .addLineToPoint: d += "L\(f(pt[0].x)) \(f(pt[0].y))"
        case .addQuadCurveToPoint: d += "Q\(f(pt[0].x)) \(f(pt[0].y)) \(f(pt[1].x)) \(f(pt[1].y))"
        case .addCurveToPoint: d += "C\(f(pt[0].x)) \(f(pt[0].y)) \(f(pt[1].x)) \(f(pt[1].y)) \(f(pt[2].x)) \(f(pt[2].y))"
        case .closeSubpath: d += "Z"
        @unknown default: break
        }
    }
    return d
}

// ---------- the icon, on a 100-unit square ----------

enum Shape {
    case rect(CGRect, r: CGFloat, fill: String, opacity: CGFloat = 1)
    case ring(CGPoint, r: CGFloat, stroke: String, width: CGFloat)
    case dot(CGPoint, r: CGFloat, fill: String)
    case path(CGPath, fill: String)
}

// The `s`: 48 units of Fraunces SemiBold, placed by its ink, not its advance, so the gaps either
// side of it are what they look like: dot, 5, s, 5, bar, the group centred in the row, and the
// letter's body centred on the row's middle line.
let s = textPath("s", size: 48)
let gap: CGFloat = 5, dotR: CGFloat = 7.5, barW: CGFloat = 18
let group = dotR * 2 + gap + s.ink.width + gap + barW
let left = 50 - group / 2
let dotX = left + dotR
let sLeft = left + dotR * 2 + gap
func moved(_ p: CGPath, _ x: CGFloat, _ y: CGFloat) -> CGPath {
    var t = CGAffineTransform(translationX: x, y: y)
    return p.copy(using: &t)!
}
let sPlaced = moved(s.path, sLeft - s.ink.minX, 50 - s.ink.midY)
let barX = sLeft + s.ink.width + gap

let icon: [Shape] = [
    .rect(CGRect(x: 2, y: 2, width: 96, height: 96), r: 22, fill: "#121519"),
    .ring(CGPoint(x: dotX, y: 17), r: 4, stroke: "#3a414a", width: 2.5),
    .rect(CGRect(x: sLeft, y: 12, width: 42, height: 10), r: 5, fill: "#1f242b"),
    .rect(CGRect(x: 8, y: 28, width: 84, height: 44), r: 12, fill: "#6b4a8f"),
    .dot(CGPoint(x: dotX, y: 50), r: dotR, fill: "#4ec96a"),
    .path(sPlaced, fill: "#f4effa"),
    .rect(CGRect(x: barX, y: 45, width: barW, height: 10), r: 5, fill: "#f4effa", opacity: 0.55),
    .ring(CGPoint(x: dotX, y: 83), r: 4, stroke: "#3a414a", width: 2.5),
    .rect(CGRect(x: sLeft, y: 78, width: 30, height: 10), r: 5, fill: "#1f242b"),
]
// The tile's hairline, drawn last so it sits over the rows' edges.
let tileEdge = (rect: CGRect(x: 2, y: 2, width: 96, height: 96), r: CGFloat(22), stroke: "#242a31", width: CGFloat(1.5))

func iconSVG(_ shapes: [Shape], at t: CGAffineTransform = .identity) -> String {
    var g = ""
    let f = { (v: CGFloat) in String(format: "%.2f", v).replacingOccurrences(of: ".00", with: "") }
    for sh in shapes {
        switch sh {
        case let .rect(r, rad, fill, op):
            let o = op < 1 ? " fill-opacity=\"\(f(op))\"" : ""
            g += "<rect x=\"\(f(r.minX))\" y=\"\(f(r.minY))\" width=\"\(f(r.width))\" height=\"\(f(r.height))\" rx=\"\(f(rad))\" fill=\"\(fill)\"\(o)/>"
        case let .ring(c, r, stroke, w):
            g += "<circle cx=\"\(f(c.x))\" cy=\"\(f(c.y))\" r=\"\(f(r))\" fill=\"none\" stroke=\"\(stroke)\" stroke-width=\"\(f(w))\"/>"
        case let .dot(c, r, fill):
            g += "<circle cx=\"\(f(c.x))\" cy=\"\(f(c.y))\" r=\"\(f(r))\" fill=\"\(fill)\"/>"
        case let .path(p, fill):
            g += "<path d=\"\(svgPath(p))\" fill=\"\(fill)\"/>"
        }
    }
    let e = tileEdge
    g += "<rect x=\"\(f(e.rect.minX))\" y=\"\(f(e.rect.minY))\" width=\"\(f(e.rect.width))\" height=\"\(f(e.rect.height))\" rx=\"\(f(e.r))\" fill=\"none\" stroke=\"\(e.stroke)\" stroke-width=\"\(f(e.width))\"/>"
    return g
}

func color(_ hex: String, _ alpha: CGFloat = 1) -> CGColor {
    let v = Int(hex.dropFirst(), radix: 16)!
    return CGColor(srgbRed: CGFloat(v >> 16 & 255) / 255, green: CGFloat(v >> 8 & 255) / 255, blue: CGFloat(v & 255) / 255, alpha: alpha)
}

/// Draw the icon into `cx` in a `size`-point square whose top-left is `origin` (y down).
func drawIcon(_ cx: CGContext, origin: CGPoint, size: CGFloat) {
    cx.saveGState()
    cx.translateBy(x: origin.x, y: origin.y)
    cx.scaleBy(x: size / 100, y: size / 100)
    for sh in icon {
        switch sh {
        case let .rect(r, rad, fill, op):
            cx.setFillColor(color(fill, op)); cx.addPath(CGPath(roundedRect: r, cornerWidth: rad, cornerHeight: rad, transform: nil)); cx.fillPath()
        case let .ring(c, r, stroke, w):
            cx.setStrokeColor(color(stroke)); cx.setLineWidth(w); cx.strokeEllipse(in: CGRect(x: c.x - r, y: c.y - r, width: 2 * r, height: 2 * r))
        case let .dot(c, r, fill):
            cx.setFillColor(color(fill)); cx.fillEllipse(in: CGRect(x: c.x - r, y: c.y - r, width: 2 * r, height: 2 * r))
        case let .path(p, fill):
            cx.setFillColor(color(fill)); cx.addPath(p); cx.fillPath()
        }
    }
    let e = tileEdge
    cx.setStrokeColor(color(e.stroke)); cx.setLineWidth(e.width)
    cx.addPath(CGPath(roundedRect: e.rect, cornerWidth: e.r, cornerHeight: e.r, transform: nil)); cx.strokePath()
    cx.restoreGState()
}

/// A `w`×`h` PNG drawn by `draw` in a y-down context.
func png(_ name: String, _ w: Int, _ h: Int, _ draw: (CGContext) -> Void) throws {
    let cx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
                       space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    cx.translateBy(x: 0, y: CGFloat(h)); cx.scaleBy(x: 1, y: -1)
    cx.setShouldAntialias(true); cx.interpolationQuality = .high
    draw(cx)
    let rep = NSBitmapImageRep(cgImage: cx.makeImage()!)
    try rep.representation(using: .png, properties: [:])!.write(to: out.appendingPathComponent(name))
}

// ---------- write ----------

let svgHead = { (w: CGFloat, h: CGFloat, label: String) in
    "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 \(Int(w)) \(Int(h))\" width=\"\(Int(w))\" height=\"\(Int(h))\" role=\"img\" aria-label=\"\(label)\">"
}
try (svgHead(100, 100, "sessio") + iconSVG(icon) + "</svg>\n").write(to: out.appendingPathComponent("icon.svg"), atomically: true, encoding: .utf8)

for size in [1024, 512, 256, 180, 64, 32] {
    try png("icon-\(size).png", size, size) { drawIcon($0, origin: .zero, size: CGFloat(size)) }
}

// The logo: the icon, then "sessio" in Fraunces SemiBold, its x-height centred on the icon.
let word = textPath("sessio", size: 60)
let xh = CTFontGetXHeight(fraunces(60))
func logoSVG(text: String) -> String {
    let iconSize: CGFloat = 72, gapW: CGFloat = 18
    let w = iconSize + gapW + word.width + 4, h = iconSize
    return svgHead(w.rounded(.up), h, "sessio")
        + "<g transform=\"scale(\(iconSize / 100))\">" + iconSVG(icon) + "</g>"
        + "<path d=\"\(svgPath(moved(word.path, iconSize + gapW, h / 2 + xh / 2)))\" fill=\"\(text)\"/></svg>\n"
}
try logoSVG(text: "#e6e9ee").write(to: out.appendingPathComponent("logo-dark.svg"), atomically: true, encoding: .utf8)
try logoSVG(text: "#16181c").write(to: out.appendingPathComponent("logo-light.svg"), atomically: true, encoding: .utf8)

// The social card link previews show: the logo large on the site's surface, and the line under it.
try png("social.png", 1200, 630) { cx in
    cx.setFillColor(color("#0b0d10")); cx.fill(CGRect(x: 0, y: 0, width: 1200, height: 630))
    let big = textPath("sessio", size: 150), bigXh = CTFontGetXHeight(fraunces(150))
    let iconSize: CGFloat = 180, gapW: CGFloat = 44
    let total = iconSize + gapW + big.width
    let x0 = (1200 - total) / 2, midY: CGFloat = 270
    drawIcon(cx, origin: CGPoint(x: x0, y: midY - iconSize / 2), size: iconSize)
    cx.setFillColor(color("#e6e9ee"))
    var t = CGAffineTransform(translationX: x0 + iconSize + gapW, y: midY + bigXh / 2)
    cx.addPath(big.path.copy(using: &t)!); cx.fillPath()
    let tag = NSAttributedString(string: "Find and resume your Claude Code sessions", attributes: [
        .font: NSFont.systemFont(ofSize: 40, weight: .regular), .foregroundColor: NSColor(cgColor: color("#8a93a0"))!,
    ])
    let line = CTLineCreateWithAttributedString(tag)
    let tw = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
    cx.saveGState(); cx.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
    cx.textPosition = CGPoint(x: (1200 - tw) / 2, y: 470); CTLineDraw(line, cx); cx.restoreGState()
}

print("wrote \(out.path)")

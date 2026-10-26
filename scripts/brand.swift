// Draw sessio's marks into docs/brand/, from one description of the selected row.
//
// Every mark is the dashboard's selected session row: a plum bar with the session's running dot in
// it. The **bar** is that row with a circle bitten out of its left end and the green dot in the
// bite. Stood on end it is the **capsule**, the app icon and logo, with the `s` under the dot.
// The **wordmark** is "sessio" with the running dot as the tittle of its i. Run across the full
// width of a page the bar is the **band** (the site draws that in CSS). The **postmark** dates
// things and is never the logo. The letters are Fraunces SemiBold (and Menlo for the postmark's
// rim), turned into outlines here, so the files look the same on a machine without the fonts.
//
//   swift scripts/brand.swift            # needs network once, for the font
//
// Everything it writes is committed; run it again only to change a mark. docs/DESIGN.md
// ("Brand") has the geometry and the reasons for it.
import AppKit
import CoreText
import Foundation

let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
let out = root.appendingPathComponent("docs/brand")
try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)

// The palette the marks use: the identity colours and the site's darkest neutral.
let plum = "#6b4a8f", green = "#4ec96a", onPlum = "#f4effa", ground = "#0c0b10"
let inkDark = "#ece8f2", inkLight = "#18151d", dimDark = "#978ea5", amber = "#e0b341"

// ---------- the fonts ----------

let fontURL = FileManager.default.temporaryDirectory.appendingPathComponent("sessio-Fraunces-600.ttf")
if !FileManager.default.fileExists(atPath: fontURL.path) {
    let css = try String(contentsOf: URL(string: "https://fonts.googleapis.com/css2?family=Fraunces:wght@600")!, encoding: .utf8)
    guard let r = css.range(of: #"https://[^)]+\.ttf"#, options: .regularExpression) else { fatalError("no ttf in Google Fonts css") }
    try Data(contentsOf: URL(string: String(css[r]))!).write(to: fontURL)
}
let desc = (CTFontManagerCreateFontDescriptorsFromURL(fontURL as CFURL) as! [CTFontDescriptor])[0]
func fraunces(_ size: CGFloat) -> CTFont { CTFontCreateWithFontDescriptor(desc, size, nil) }
func menlo(_ size: CGFloat) -> CTFont { CTFontCreateWithName("Menlo-Regular" as CFString, size, nil) }

/// `text` as one path, origin at the left end of its baseline, y down (SVG's way round), with
/// each glyph's own ink box so a caller can place something over one letter.
func textPath(_ text: String, font: CTFont) -> (path: CGPath, width: CGFloat, ink: CGRect, glyphs: [CGRect]) {
    let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: [.init(kCTFontAttributeName as String): font]))
    let path = CGMutablePath()
    var boxes: [CGRect] = []
    for run in CTLineGetGlyphRuns(line) as! [CTRun] {
        let n = CTRunGetGlyphCount(run)
        var glyphs = [CGGlyph](repeating: 0, count: n), pos = [CGPoint](repeating: .zero, count: n)
        CTRunGetGlyphs(run, CFRange(), &glyphs); CTRunGetPositions(run, CFRange(), &pos)
        for i in 0..<n {
            guard let g = CTFontCreatePathForGlyph(font, glyphs[i], nil) else { boxes.append(.null); continue }
            let t = CGAffineTransform(a: 1, b: 0, c: 0, d: -1, tx: pos[i].x, ty: 0)
            path.addPath(g, transform: t)
            boxes.append(g.boundingBoxOfPath.applying(t))
        }
    }
    return (path, CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil)), path.boundingBoxOfPath, boxes)
}
func textPath(_ text: String, size: CGFloat) -> (path: CGPath, width: CGFloat, ink: CGRect, glyphs: [CGRect]) {
    textPath(text, font: fraunces(size))
}

func moved(_ p: CGPath, _ x: CGFloat, _ y: CGFloat) -> CGPath {
    var t = CGAffineTransform(translationX: x, y: y)
    return p.copy(using: &t)!
}

let num = { (v: CGFloat) in String(format: "%.2f", v).replacingOccurrences(of: ".00", with: "") }

func svgPath(_ p: CGPath) -> String {
    var d = ""
    p.applyWithBlock { e in
        let pt = e.pointee.points
        switch e.pointee.type {
        case .moveToPoint: d += "M\(num(pt[0].x)) \(num(pt[0].y))"
        case .addLineToPoint: d += "L\(num(pt[0].x)) \(num(pt[0].y))"
        case .addQuadCurveToPoint: d += "Q\(num(pt[0].x)) \(num(pt[0].y)) \(num(pt[1].x)) \(num(pt[1].y))"
        case .addCurveToPoint: d += "C\(num(pt[0].x)) \(num(pt[0].y)) \(num(pt[1].x)) \(num(pt[1].y)) \(num(pt[2].x)) \(num(pt[2].y))"
        case .closeSubpath: d += "Z"
        @unknown default: break
        }
    }
    return d
}

func color(_ hex: String, _ alpha: CGFloat = 1) -> CGColor {
    let v = Int(hex.dropFirst(), radix: 16)!
    return CGColor(srgbRed: CGFloat(v >> 16 & 255) / 255, green: CGFloat(v >> 8 & 255) / 255, blue: CGFloat(v & 255) / 255, alpha: alpha)
}

// ---------- shapes: each mark is a list of these, drawn the same way into SVG and into PNG ----------

enum Shape {
    /// A filled path; `evenOdd` lets a bite be a second subpath cut out of the first.
    case fill(CGPath, String, evenOdd: Bool = false)
    case stroke(CGPath, String, width: CGFloat)
}

func capsule(_ r: CGRect) -> CGPath {
    let rad = min(r.width, r.height) / 2
    return CGPath(roundedRect: r, cornerWidth: rad, cornerHeight: rad, transform: nil)
}
func circle(_ c: CGPoint, _ r: CGFloat) -> CGPath {
    CGPath(ellipseIn: CGRect(x: c.x - r, y: c.y - r, width: 2 * r, height: 2 * r), transform: nil)
}
/// `shape` with a circle bitten out of it, the running dot set inside the bite.
func bitten(_ shape: CGPath, at c: CGPoint, bite: CGFloat, dot: CGFloat?, fill: String) -> [Shape] {
    let p = CGMutablePath(); p.addPath(shape); p.addPath(circle(c, bite))
    var s: [Shape] = [.fill(p, fill, evenOdd: true)]
    if let dot { s.append(.fill(circle(c, dot), green)) }
    return s
}

func svg(_ shapes: [Shape]) -> String {
    shapes.map {
        switch $0 {
        case let .fill(p, c, eo): return "<path d=\"\(svgPath(p))\" fill=\"\(c)\"\(eo ? " fill-rule=\"evenodd\"" : "")/>"
        case let .stroke(p, c, w): return "<path d=\"\(svgPath(p))\" fill=\"none\" stroke=\"\(c)\" stroke-width=\"\(num(w))\"/>"
        }
    }.joined()
}

func draw(_ cx: CGContext, _ shapes: [Shape], origin: CGPoint = .zero, scale: CGFloat = 1) {
    cx.saveGState()
    cx.translateBy(x: origin.x, y: origin.y); cx.scaleBy(x: scale, y: scale)
    for s in shapes {
        switch s {
        case let .fill(p, c, eo): cx.setFillColor(color(c)); cx.addPath(p); cx.fillPath(using: eo ? .evenOdd : .winding)
        case let .stroke(p, c, w): cx.setStrokeColor(color(c)); cx.setLineWidth(w); cx.addPath(p); cx.strokePath()
        }
    }
    cx.restoreGState()
}

// ---------- the marks ----------

/// The `s`, placed by its ink: its body centred on `c`, `height` units of ink tall.
func sAt(_ c: CGPoint, height: CGFloat) -> CGPath {
    let probe = textPath("s", size: 100)
    let s = textPath("s", size: 100 * height / probe.ink.height)
    return moved(s.path, c.x - s.ink.midX, c.y - s.ink.midY)
}

// The capsule, on a 100-unit square: the row stood on end, the dot in a bite at its top, the `s`
// below it. The bite is a ring of ground around the dot, as the row draws the dot.
let capRect = CGRect(x: 22, y: 4, width: 56, height: 92)
let capDot = CGPoint(x: 50, y: 31)
func capsuleMark(withS: Bool) -> [Shape] {
    var s = bitten(capsule(capRect), at: capDot, bite: 14, dot: 9.5, fill: plum)
    if withS { s.append(.fill(sAt(CGPoint(x: 50, y: 65), height: 34), onPlum)) }
    return s
}
// Small sizes (16, 32): no letter to blur, a bigger dot.
let capsuleSmall = bitten(capsule(capRect), at: CGPoint(x: 50, y: 34), bite: 18, dot: 12.5, fill: plum)
// The outline capsule for the logo: the row's edge drawn, the dot and the `s` in plum ink, so it
// reads on dark and on light without a tile.
func capsuleOutline(ink: String) -> [Shape] {
    [.stroke(capsule(capRect.insetBy(dx: 3.5, dy: 3.5)), plum, width: 7),
     .fill(circle(capDot, 9.5), green),
     .fill(sAt(CGPoint(x: 50, y: 66), height: 29), ink)]
}

// The bar, on a 100×40 field: one row, one bite, one dot.
let barShapes = bitten(capsule(CGRect(x: 0, y: 0, width: 100, height: 40)), at: CGPoint(x: 20, y: 20), bite: 11, dot: 7.5, fill: plum)

// The wordmark: "sessio" with a dotless i, the running dot set where its tittle was.
let wordSize: CGFloat = 60
let word = textPath("sess\u{0131}o", size: wordSize)
let dotted = textPath("i", size: wordSize).ink, dotless = textPath("\u{0131}", size: wordSize).ink
// The tittle's own box: the part of the i's ink above the dotless i's.
let tittleH = dotless.minY - dotted.minY
let tittleR = tittleH * 0.5
let iBox = word.glyphs[4]
let tittle = CGPoint(x: iBox.midX, y: iBox.minY - tittleH * 0.62)
func wordmark(ink: String, dx: CGFloat = 0, dy: CGFloat = 0) -> [Shape] {
    [.fill(moved(word.path, dx, dy), ink),
     .fill(circle(CGPoint(x: tittle.x + dx, y: tittle.y + dy), tittleR * 1.12), green)]
}
// Ink extents of the wordmark, tittle included, for placing it.
let wordTop = min(word.ink.minY, tittle.y - tittleR * 1.12), wordBottom = word.ink.maxY

// ---------- the postmark ----------

/// A postmark centred on `c`, radius `r`: two rings, the rim text set glyph by glyph round the
/// circle in Menlo, the date in Fraunces in the middle and the running dot under it.
func postmark(_ c: CGPoint, r: CGFloat, ink: String, date: (String, String)) -> [Shape] {
    var s: [Shape] = [.stroke(circle(c, r), ink, width: r * 0.04), .stroke(circle(c, r * 0.62), ink, width: r * 0.04)]
    let rim = "SESSIO · ~/.CLAUDE/PROJECTS · ", font = menlo(r * 0.155)
    let rimR = r * 0.81
    let step = 2 * CGFloat.pi / CGFloat(rim.count)
    for (i, ch) in rim.enumerated() where ch != " " {
        let g = textPath(String(ch), font: font)
        let a = -CGFloat.pi / 2 + CGFloat(i) * step
        var t = CGAffineTransform(translationX: c.x + rimR * cos(a), y: c.y + rimR * sin(a))
            .rotated(by: a + .pi / 2).translatedBy(x: -g.ink.midX, y: -g.ink.midY)
        s.append(.fill(g.path.copy(using: &t)!, ink))
    }
    let d1 = textPath(date.0, size: r * 0.27), d2 = textPath(date.1, font: menlo(r * 0.15))
    s.append(.fill(moved(d1.path, c.x - d1.ink.midX, c.y - r * 0.05), ink))
    s.append(.fill(moved(d2.path, c.x - d2.ink.midX, c.y + r * 0.22), ink))
    s.append(.fill(circle(CGPoint(x: c.x, y: c.y + r * 0.42), r * 0.065), green))
    return s
}

// ---------- write ----------

func png(_ name: String, _ w: Int, _ h: Int, _ body: (CGContext) -> Void) throws {
    let cx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
                       space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    cx.translateBy(x: 0, y: CGFloat(h)); cx.scaleBy(x: 1, y: -1)
    cx.setShouldAntialias(true); cx.interpolationQuality = .high
    body(cx)
    let rep = NSBitmapImageRep(cgImage: cx.makeImage()!)
    try rep.representation(using: .png, properties: [:])!.write(to: out.appendingPathComponent(name))
}
func writeSVG(_ name: String, w: CGFloat, h: CGFloat, label: String, _ body: String) throws {
    let head = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 \(num(w)) \(num(h))\" width=\"\(num(w))\" height=\"\(num(h))\" role=\"img\" aria-label=\"\(label)\">"
    try (head + body + "</svg>\n").write(to: out.appendingPathComponent(name), atomically: true, encoding: .utf8)
}

// The icon: the capsule on a transparent square (favicons, the notification image).
try writeSVG("icon.svg", w: 100, h: 100, label: "sessio", svg(capsuleMark(withS: true)))
try writeSVG("icon-small.svg", w: 100, h: 100, label: "sessio", svg(capsuleSmall))
for size in [512, 256, 64] {
    try png("icon-\(size).png", size, size) { draw($0, capsuleMark(withS: true), scale: CGFloat(size) / 100) }
}
try png("icon-32.png", 32, 32) { draw($0, capsuleSmall, scale: 0.32) }
// Apple's touch icon and the 1024 app icon must be opaque: the capsule on the darkest ground,
// which the platform rounds itself.
for size in [1024, 180] {
    try png("icon-\(size).png", size, size) { cx in
        cx.setFillColor(color(ground)); cx.fill(CGRect(x: 0, y: 0, width: size, height: size))
        let k = CGFloat(size) * 0.78 / 100
        draw(cx, capsuleMark(withS: true), origin: CGPoint(x: CGFloat(size) * 0.11, y: CGFloat(size) * 0.11), scale: k)
    }
}

try writeSVG("bar.svg", w: 100, h: 40, label: "sessio", svg(barShapes))

// The wordmark alone, and the logo: the outline capsule beside the wordmark, the capsule as tall
// as the wordmark's ink from tittle to baseline plus a little, the word on the capsule's middle.
let wmW = word.width + 2, wmH = wordBottom - wordTop + 4
try writeSVG("wordmark-dark.svg", w: wmW, h: wmH, label: "sessio", svg(wordmark(ink: inkDark, dx: 1, dy: 2 - wordTop)))
try writeSVG("wordmark-light.svg", w: wmW, h: wmH, label: "sessio", svg(wordmark(ink: inkLight, dx: 1, dy: 2 - wordTop)))
func logo(ink: String) -> (String, CGFloat, CGFloat) {
    let capH: CGFloat = 76, k = capH / 92, capW = 56 * k, gap: CGFloat = 16
    let h = capH + 4
    let mid = h / 2
    var g = "<g transform=\"translate(\(num(-22 * k + 2)) \(num(-4 * k + 2))) scale(\(num(k)))\">" + svg(capsuleOutline(ink: ink)) + "</g>"
    let dy = mid - (wordTop + wordBottom) / 2
    g += svg(wordmark(ink: ink, dx: capW + 2 + gap, dy: dy))
    return (g, capW + 2 + gap + word.width + 4, h)
}
for (name, ink) in [("logo-dark.svg", inkDark), ("logo-light.svg", inkLight)] {
    let (g, w, h) = logo(ink: ink)
    try writeSVG(name, w: w, h: h, label: "sessio", g)
}

// The postmark carries the date of the newest dated release in CHANGELOG.md, so run this as part
// of a release, after the version's heading has its date. Plum ink for light pages, a light plum
// for dark ones.
let today = { () -> (String, String) in
    let log = (try? String(contentsOf: root.appendingPathComponent("CHANGELOG.md"), encoding: .utf8)) ?? ""
    let iso = DateFormatter(); iso.locale = Locale(identifier: "en_US_POSIX"); iso.dateFormat = "yyyy-MM-dd"
    var when = Date()
    // The first release heading that carries a date: `## Unreleased` has none.
    for line in log.split(separator: "\n") where line.hasPrefix("## ") {
        if let r = line.range(of: #"\d{4}-\d{2}-\d{2}"#, options: .regularExpression), let d = iso.date(from: String(line[r])) {
            when = d
            break
        }
    }
    let f = DateFormatter(); f.locale = Locale(identifier: "en_US_POSIX")
    f.dateFormat = "dd MMM"; let a = f.string(from: when).uppercased()
    f.dateFormat = "yyyy"; return (a, f.string(from: when))
}()
try writeSVG("postmark.svg", w: 120, h: 120, label: "sessio, \(today.0) \(today.1)", svg(postmark(CGPoint(x: 60, y: 60), r: 56, ink: plum, date: today)))
try writeSVG("postmark-dark.svg", w: 120, h: 120, label: "sessio, \(today.0) \(today.1)", svg(postmark(CGPoint(x: 60, y: 60), r: 56, ink: "#c9b6e4", date: today)))

// The social card: the band across the top, the wordmark, the promise, the postmark, and the
// selected row the promise is about.
try png("social.png", 1200, 630) { cx in
    cx.setFillColor(color(ground)); cx.fill(CGRect(x: 0, y: 0, width: 1200, height: 630))
    // the band: plum to the right edge, its left end bitten with the dot in it; the green rule
    let band = CGMutablePath(); band.addRect(CGRect(x: 0, y: 0, width: 1200, height: 44))
    band.addPath(circle(CGPoint(x: 64, y: 22), 15))
    draw(cx, [.fill(band, plum, evenOdd: true), .fill(circle(CGPoint(x: 64, y: 22), 10), green),
              .fill(CGPath(rect: CGRect(x: 0, y: 44, width: 1200, height: 8), transform: nil), green)])
    // the wordmark, then the promise in two lines
    let k: CGFloat = 1.25
    draw(cx, wordmark(ink: inkDark), origin: CGPoint(x: 80, y: 150 - wordTop * k), scale: k)
    for (i, line) in ["Find the session you left,", "and pick it back up."].enumerated() {
        let t = textPath(line, size: 64)
        draw(cx, [.fill(t.path, inkDark)], origin: CGPoint(x: 80, y: 330 + CGFloat(i) * 78))
    }
    draw(cx, postmark(CGPoint(x: 1000, y: 290), r: 120, ink: "#c9b6e4", date: today))
    // the selected row
    let row = CGRect(x: 80, y: 490, width: 1040, height: 72)
    draw(cx, [.fill(CGPath(roundedRect: row, cornerWidth: 12, cornerHeight: 12, transform: nil), plum)])
    let mark = textPath("\u{25C6}", font: menlo(30)), title = textPath("fix login redirect loop", font: menlo(30))
    let hint = textPath("\u{21B5} resume", font: menlo(30))
    let base = row.midY + 10
    draw(cx, [.fill(mark.path, amber)], origin: CGPoint(x: row.minX + 26, y: base))
    draw(cx, [.fill(title.path, onPlum)], origin: CGPoint(x: row.minX + 72, y: base))
    draw(cx, [.fill(hint.path, onPlum)], origin: CGPoint(x: row.maxX - 26 - hint.width, y: base))
}

print("wrote \(out.path)")

#!/usr/bin/env swift
/// Draw the Daggy app icon at 1024px and assemble AppIcon.icns.

import AppKit
import Foundation

let canvas: CGFloat = 1024
let graphite = (30.0 / 255.0, 30.0 / 255.0, 32.0 / 255.0)

struct Slice {
    var share: CGFloat
    var color: (CGFloat, CGFloat, CGFloat)
    var children: [Slice]
}

func shade(_ color: (CGFloat, CGFloat, CGFloat), lightness: CGFloat) -> (CGFloat, CGFloat, CGFloat) {
    (
        min(1, max(0, color.0 * lightness)),
        min(1, max(0, color.1 * lightness)),
        min(1, max(0, color.2 * lightness))
    )
}

let blue: (CGFloat, CGFloat, CGFloat) = (62 / 255, 142 / 255, 247 / 255)
let pink: (CGFloat, CGFloat, CGFloat) = (242 / 255, 86 / 255, 140 / 255)
let teal: (CGFloat, CGFloat, CGFloat) = (62 / 255, 198 / 255, 189 / 255)
let orange: (CGFloat, CGFloat, CGFloat) = (242 / 255, 145 / 255, 62 / 255)

let tree: [Slice] = [
    Slice(share: 0.42, color: blue, children: [
        Slice(share: 0.42, color: shade(blue, lightness: 0.82), children: []),
    ]),
    Slice(share: 0.26, color: pink, children: [
        Slice(share: 0.26, color: shade(pink, lightness: 0.84), children: []),
    ]),
    Slice(share: 0.18, color: teal, children: [
        Slice(share: 0.18, color: shade(teal, lightness: 0.82), children: []),
    ]),
    Slice(share: 0.14, color: orange, children: [
        Slice(share: 0.14, color: shade(orange, lightness: 0.84), children: []),
    ]),
]

func fill(_ ctx: CGContext, _ rgb: (CGFloat, CGFloat, CGFloat)) {
    ctx.setFillColor(CGColor(red: rgb.0, green: rgb.1, blue: rgb.2, alpha: 1))
}

func addAnnulus(
    _ path: CGMutablePath,
    center: CGPoint,
    inner: CGFloat,
    outer: CGFloat,
    start: CGFloat,
    end: CGFloat
) {
    path.addArc(center: center, radius: outer, startAngle: start, endAngle: end, clockwise: false)
    path.addArc(center: center, radius: inner, startAngle: end, endAngle: start, clockwise: true)
    path.closeSubpath()
}

func drawRing(
    _ ctx: CGContext,
    center: CGPoint,
    inner: CGFloat,
    outer: CGFloat,
    slices: [Slice],
    sliceGap: CGFloat
) {
    let total = slices.reduce(CGFloat(0)) { $0 + $1.share }
    var angle = -CGFloat.pi / 2
    for slice in slices {
        let span = (slice.share / total) * 2 * .pi
        let start = angle + sliceGap / 2
        let end = angle + span - sliceGap / 2
        if end > start {
            let path = CGMutablePath()
            addAnnulus(path, center: center, inner: inner, outer: outer, start: start, end: end)
            ctx.addPath(path)
            fill(ctx, slice.color)
            ctx.fillPath()
        }
        angle += span
    }
}

func renderPNG(to url: URL) throws {
    let colorSpace = CGColorSpaceCreateDeviceRGB()
    guard let ctx = CGContext(
        data: nil,
        width: Int(canvas),
        height: Int(canvas),
        bitsPerComponent: 8,
        bytesPerRow: 0,
        space: colorSpace,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    ) else {
        throw NSError(domain: "daggy-icon", code: 1)
    }
    ctx.setFillColor(CGColor(red: graphite.0, green: graphite.1, blue: graphite.2, alpha: 1))
    ctx.fill(CGRect(x: 0, y: 0, width: canvas, height: canvas))

    let center = CGPoint(x: canvas / 2, y: canvas / 2)
    let maxR = canvas * 0.42
    let hole = maxR * 0.34
    let ringGap = canvas * 0.012
    let sliceGap: CGFloat = 0.045
    let rings = 2
    let ringWidth = (maxR - hole - ringGap * CGFloat(rings - 1)) / CGFloat(rings)

    let innerSlices = tree
    let outerSlices = tree.map { parent in
        parent.children.first ?? Slice(share: parent.share, color: shade(parent.color, lightness: 0.84), children: [])
    }

    let bands = [innerSlices, outerSlices]
    for (index, slices) in bands.enumerated() {
        let inner = hole + CGFloat(index) * (ringWidth + ringGap)
        let outer = inner + ringWidth
        drawRing(ctx, center: center, inner: inner, outer: outer, slices: slices, sliceGap: sliceGap)
    }

    guard let image = ctx.makeImage() else {
        throw NSError(domain: "daggy-icon", code: 2)
    }
    let rep = NSBitmapImageRep(cgImage: image)
    guard let png = rep.representation(using: .png, properties: [:]) else {
        throw NSError(domain: "daggy-icon", code: 3)
    }
    try png.write(to: url)
}

let root = URL(fileURLWithPath: #file)
    .deletingLastPathComponent()
    .deletingLastPathComponent()
let assets = root.appendingPathComponent("crates/daggy-app/Assets")
try FileManager.default.createDirectory(at: assets, withIntermediateDirectories: true)
let pngURL = assets.appendingPathComponent("AppIcon.png")
try renderPNG(to: pngURL)

let iconset = assets.appendingPathComponent("AppIcon.iconset")
try? FileManager.default.removeItem(at: iconset)
try FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)

let sizes: [(String, Int)] = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
]

for (name, pixels) in sizes {
    let dest = iconset.appendingPathComponent(name)
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/sips")
    process.arguments = ["-z", "\(pixels)", "\(pixels)", pngURL.path, "--out", dest.path]
    try process.run()
    process.waitUntilExit()
    if process.terminationStatus != 0 {
        throw NSError(domain: "daggy-icon", code: 4)
    }
}

let icns = assets.appendingPathComponent("AppIcon.icns")
let iconutil = Process()
iconutil.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
iconutil.arguments = ["-c", "icns", iconset.path, "-o", icns.path]
try iconutil.run()
iconutil.waitUntilExit()
if iconutil.terminationStatus != 0 {
    throw NSError(domain: "daggy-icon", code: 5)
}

print("Wrote \(pngURL.path)")
print("Wrote \(icns.path)")

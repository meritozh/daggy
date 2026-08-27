#!/usr/bin/env swift
/// Draw the Daggy app icon at 1024px and assemble AppIcon.icns.
///
/// The mark is a three-ring sunburst: children occupy their parent's angle
/// and may split, with gutters of constant pixel width (not constant angle).

import AppKit
import Foundation

let canvas: CGFloat = 1024
let graphite = rgb(0x1E1E20)
let rings = 3
let gutter: CGFloat = 16

typealias RGB = (CGFloat, CGFloat, CGFloat)

func rgb(_ hex: UInt32) -> RGB {
    (
        CGFloat((hex >> 16) & 0xFF) / 255,
        CGFloat((hex >> 8) & 0xFF) / 255,
        CGFloat(hex & 0xFF) / 255
    )
}

struct Node {
    var share: CGFloat
    var children: [Node]
}

struct Wedge {
    var depth: Int
    var start: CGFloat
    var end: CGFloat
    var color: RGB
}

let families: [RGB] = [
    rgb(0x3E8EF7),
    rgb(0xF2568C),
    rgb(0x3EC6BD),
    rgb(0xF2913E),
]

let tree: [Node] = [
    Node(share: 0.42, children: [
        Node(share: 0.50, children: [
            Node(share: 0.58, children: []),
            Node(share: 0.42, children: []),
        ]),
        Node(share: 0.31, children: [
            Node(share: 1.00, children: []),
        ]),
        Node(share: 0.19, children: [
            Node(share: 0.56, children: []),
            Node(share: 0.44, children: []),
        ]),
    ]),
    Node(share: 0.26, children: [
        Node(share: 0.62, children: [
            Node(share: 0.60, children: []),
            Node(share: 0.40, children: []),
        ]),
        Node(share: 0.38, children: [
            Node(share: 1.00, children: []),
        ]),
    ]),
    Node(share: 0.18, children: [
        Node(share: 0.55, children: [
            Node(share: 1.00, children: []),
        ]),
        Node(share: 0.45, children: [
            Node(share: 0.52, children: []),
            Node(share: 0.48, children: []),
        ]),
    ]),
    Node(share: 0.14, children: [
        Node(share: 0.64, children: [
            Node(share: 0.55, children: []),
            Node(share: 0.45, children: []),
        ]),
        Node(share: 0.36, children: [
            Node(share: 1.00, children: []),
        ]),
    ]),
]

func rgbToHsl(_ color: RGB) -> (CGFloat, CGFloat, CGFloat) {
    let maxV = max(color.0, max(color.1, color.2))
    let minV = min(color.0, min(color.1, color.2))
    let lightness = (maxV + minV) / 2
    if maxV - minV < 1e-6 {
        return (0, 0, lightness)
    }
    let delta = maxV - minV
    let saturation = lightness > 0.5
        ? delta / (2 - maxV - minV)
        : delta / (maxV + minV)
    let hue: CGFloat
    if abs(maxV - color.0) < 1e-6 {
        var value = (color.1 - color.2) / delta
        if color.1 < color.2 {
            value += 6
        }
        hue = value / 6
    } else if abs(maxV - color.1) < 1e-6 {
        hue = ((color.2 - color.0) / delta + 2) / 6
    } else {
        hue = ((color.0 - color.1) / delta + 4) / 6
    }
    return (hue, saturation, lightness)
}

func hslToRgb(_ hue: CGFloat, _ saturation: CGFloat, _ lightness: CGFloat) -> RGB {
    if saturation < 1e-6 {
        return (lightness, lightness, lightness)
    }
    let q = lightness < 0.5
        ? lightness * (1 + saturation)
        : lightness + saturation - lightness * saturation
    let p = 2 * lightness - q
    func channel(_ t: CGFloat) -> CGFloat {
        var t = t
        if t < 0 { t += 1 }
        if t > 1 { t -= 1 }
        if t < 1 / 6 { return p + (q - p) * 6 * t }
        if t < 1 / 2 { return q }
        if t < 2 / 3 { return p + (q - p) * (2 / 3 - t) * 6 }
        return p
    }
    return (channel(hue + 1 / 3), channel(hue), channel(hue - 1 / 3))
}

func ringShade(base: RGB, depth: Int, sibling: Int) -> RGB {
    var (hue, saturation, lightness) = rgbToHsl(base)
    let siblingNudge: CGFloat = depth == 0 ? 0 : (CGFloat(sibling % 3) - 1) * 0.045
    let depthShift: CGFloat = lightness >= 0.48
        ? -0.055 * CGFloat(depth)
        : 0.05 * CGFloat(depth)
    lightness = min(0.64, max(0.28, lightness + depthShift + siblingNudge))
    if saturation > 0.08 {
        saturation = min(0.88, max(0.40, saturation + 0.045 * CGFloat(depth)))
    }
    return hslToRgb(hue, saturation, lightness)
}

func flatten(
    nodes: [Node],
    start: CGFloat,
    end: CGFloat,
    depth: Int,
    family: Int,
    wedges: inout [Wedge]
) {
    if depth >= rings {
        return
    }
    let total = nodes.reduce(CGFloat(0)) { $0 + $1.share }
    guard total > 0 else { return }
    var angle = start
    for (index, node) in nodes.enumerated() {
        let span = (node.share / total) * (end - start)
        let next = angle + span
        let familyIndex = depth == 0 ? index : family
        wedges.append(
            Wedge(
                depth: depth,
                start: angle,
                end: next,
                color: ringShade(base: families[familyIndex], depth: depth, sibling: index)
            )
        )
        let children = node.children.isEmpty && depth + 1 < rings
            ? [Node(share: 1, children: [])]
            : node.children
        flatten(
            nodes: children,
            start: angle,
            end: next,
            depth: depth + 1,
            family: familyIndex,
            wedges: &wedges
        )
        angle = next
    }
}

func fill(_ ctx: CGContext, _ color: RGB) {
    ctx.setFillColor(CGColor(red: color.0, green: color.1, blue: color.2, alpha: 1))
}

func angularHalf(gap: CGFloat, radius: CGFloat) -> CGFloat {
    let half = gap / 2
    if radius <= half {
        return .pi
    }
    return asin(half / radius)
}

func addSunburstWedge(
    _ path: CGMutablePath,
    center: CGPoint,
    inner: CGFloat,
    outer: CGFloat,
    start: CGFloat,
    end: CGFloat,
    gap: CGFloat,
    insetInner: Bool,
    insetOuter: Bool
) {
    let half = gap / 2
    let rInner = insetInner ? inner + half : inner
    let rOuter = insetOuter ? outer - half : outer
    guard rOuter > rInner + 1 else { return }

    let innerHalf = angularHalf(gap: gap, radius: rInner)
    let outerHalf = angularHalf(gap: gap, radius: rOuter)
    let outerStart = start + outerHalf
    let outerEnd = end - outerHalf
    let innerStart = start + innerHalf
    let innerEnd = end - innerHalf
    guard outerEnd > outerStart + 0.002, innerEnd > innerStart + 0.002 else { return }

    path.addArc(
        center: center,
        radius: rOuter,
        startAngle: outerStart,
        endAngle: outerEnd,
        clockwise: false
    )
    path.addArc(
        center: center,
        radius: rInner,
        startAngle: innerEnd,
        endAngle: innerStart,
        clockwise: true
    )
    path.closeSubpath()
}

func renderPNG(to url: URL) throws {
    let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
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
    ctx.setShouldAntialias(true)
    ctx.interpolationQuality = .high
    fill(ctx, graphite)
    ctx.fill(CGRect(x: 0, y: 0, width: canvas, height: canvas))

    var wedges: [Wedge] = []
    flatten(
        nodes: tree,
        start: -.pi / 2,
        end: -.pi / 2 + 2 * .pi,
        depth: 0,
        family: 0,
        wedges: &wedges
    )

    let center = CGPoint(x: canvas / 2, y: canvas / 2)
    let maxR = canvas * 0.42
    let hole = maxR * 0.30
    let ringWidth = (maxR - hole) / CGFloat(rings)

    for wedge in wedges {
        let inner = hole + CGFloat(wedge.depth) * ringWidth
        let outer = inner + ringWidth
        let path = CGMutablePath()
        addSunburstWedge(
            path,
            center: center,
            inner: inner,
            outer: outer,
            start: wedge.start,
            end: wedge.end,
            gap: gutter,
            insetInner: wedge.depth > 0,
            insetOuter: wedge.depth + 1 < rings
        )
        ctx.addPath(path)
        fill(ctx, wedge.color)
        ctx.fillPath()
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

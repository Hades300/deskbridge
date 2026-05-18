#!/usr/bin/env swift
import AppKit
import Foundation

let fileManager = FileManager.default
let scriptArgument = CommandLine.arguments[0]
let scriptPath = scriptArgument.hasPrefix("/")
    ? scriptArgument
    : URL(fileURLWithPath: fileManager.currentDirectoryPath)
        .appendingPathComponent(scriptArgument)
        .standardizedFileURL
        .path
let rootURL = URL(fileURLWithPath: scriptPath)
    .deletingLastPathComponent()
    .deletingLastPathComponent()
let resourcesURL = rootURL.appendingPathComponent("apps/macos/Resources", isDirectory: true)
let buildURL = rootURL.appendingPathComponent("build", isDirectory: true)
let iconsetURL = buildURL.appendingPathComponent("DeskBridge.iconset", isDirectory: true)

try fileManager.createDirectory(at: resourcesURL, withIntermediateDirectories: true)
try fileManager.createDirectory(at: buildURL, withIntermediateDirectories: true)
try? fileManager.removeItem(at: iconsetURL)
try fileManager.createDirectory(at: iconsetURL, withIntermediateDirectories: true)

func imageData(
    pixels: Int,
    draw: (CGFloat) -> Void
) throws -> Data {
    guard let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: pixels,
        pixelsHigh: pixels,
        bitsPerSample: 8,
        samplesPerPixel: 4,
        hasAlpha: true,
        isPlanar: false,
        colorSpaceName: .deviceRGB,
        bytesPerRow: 0,
        bitsPerPixel: 0
    ) else {
        throw NSError(domain: "DeskBridgeIcon", code: 1)
    }

    rep.size = NSSize(width: pixels, height: pixels)
    guard let context = NSGraphicsContext(bitmapImageRep: rep) else {
        throw NSError(domain: "DeskBridgeIcon", code: 2)
    }

    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = context
    context.cgContext.setAllowsAntialiasing(true)
    context.cgContext.setShouldAntialias(true)
    NSColor.clear.setFill()
    NSRect(x: 0, y: 0, width: pixels, height: pixels).fill()
    draw(CGFloat(pixels) / 1024.0)
    NSGraphicsContext.restoreGraphicsState()

    guard let data = rep.representation(using: .png, properties: [:]) else {
        throw NSError(domain: "DeskBridgeIcon", code: 3)
    }
    return data
}

func rect(_ x: CGFloat, _ y: CGFloat, _ width: CGFloat, _ height: CGFloat, _ scale: CGFloat) -> NSRect {
    NSRect(x: x * scale, y: y * scale, width: width * scale, height: height * scale)
}

func color(_ red: CGFloat, _ green: CGFloat, _ blue: CGFloat, _ alpha: CGFloat = 1.0) -> NSColor {
    NSColor(calibratedRed: red, green: green, blue: blue, alpha: alpha)
}

func drawGlow(in frame: NSRect, color glowColor: NSColor) {
    let path = NSBezierPath(ovalIn: frame)
    glowColor.setFill()
    path.fill()
}

func drawScreen(
    frame: NSRect,
    cornerRadius: CGFloat,
    fillStart: NSColor,
    fillEnd: NSColor,
    border: NSColor,
    highlight: NSColor
) {
    let path = NSBezierPath(roundedRect: frame, xRadius: cornerRadius, yRadius: cornerRadius)

    let shadow = NSShadow()
    shadow.shadowOffset = NSSize(width: 0, height: -8 * frame.width / 330)
    shadow.shadowBlurRadius = 28 * frame.width / 330
    shadow.shadowColor = NSColor.black.withAlphaComponent(0.40)
    shadow.set()

    NSGradient(colors: [fillStart, fillEnd])?.draw(in: path, angle: -32)
    NSShadow().set()

    border.setStroke()
    path.lineWidth = max(2, frame.width / 72)
    path.stroke()

    let shine = NSBezierPath(
        roundedRect: frame.insetBy(dx: frame.width * 0.06, dy: frame.height * 0.08),
        xRadius: cornerRadius * 0.62,
        yRadius: cornerRadius * 0.62
    )
    highlight.withAlphaComponent(0.11).setFill()
    shine.fill()
}

func drawAppIcon(scale: CGFloat) {
    let background = NSBezierPath(
        roundedRect: rect(64, 64, 896, 896, scale),
        xRadius: 212 * scale,
        yRadius: 212 * scale
    )
    NSGradient(colors: [
        color(0.045, 0.060, 0.085),
        color(0.060, 0.072, 0.130),
        color(0.025, 0.040, 0.052),
    ])?.draw(in: background, angle: -38)

    drawGlow(
        in: rect(80, 560, 500, 300, scale),
        color: color(0.48, 0.95, 0.62, 0.18)
    )
    drawGlow(
        in: rect(445, 170, 470, 360, scale),
        color: color(0.30, 0.82, 1.00, 0.22)
    )

    let bridgeShadow = NSShadow()
    bridgeShadow.shadowBlurRadius = 38 * scale
    bridgeShadow.shadowColor = color(0.56, 1.00, 0.62, 0.42)
    bridgeShadow.set()

    let bridge = NSBezierPath()
    bridge.move(to: NSPoint(x: 420 * scale, y: 608 * scale))
    bridge.curve(
        to: NSPoint(x: 602 * scale, y: 402 * scale),
        controlPoint1: NSPoint(x: 515 * scale, y: 630 * scale),
        controlPoint2: NSPoint(x: 530 * scale, y: 378 * scale)
    )
    bridge.lineCapStyle = .round
    bridge.lineJoinStyle = .round
    bridge.lineWidth = 82 * scale
    color(0.42, 0.96, 0.58, 0.22).setStroke()
    bridge.stroke()

    NSShadow().set()
    bridge.lineWidth = 34 * scale
    color(0.68, 1.00, 0.34, 0.92).setStroke()
    bridge.stroke()
    bridge.lineWidth = 14 * scale
    color(0.34, 0.88, 1.00, 0.95).setStroke()
    bridge.stroke()

    drawScreen(
        frame: rect(155, 535, 338, 238, scale),
        cornerRadius: 56 * scale,
        fillStart: color(0.11, 0.22, 0.20),
        fillEnd: color(0.055, 0.100, 0.105),
        border: color(0.62, 1.00, 0.52, 0.90),
        highlight: .white
    )
    drawScreen(
        frame: rect(530, 280, 338, 238, scale),
        cornerRadius: 56 * scale,
        fillStart: color(0.10, 0.17, 0.30),
        fillEnd: color(0.050, 0.070, 0.120),
        border: color(0.36, 0.84, 1.00, 0.92),
        highlight: .white
    )

    let keyDeck = NSBezierPath(
        roundedRect: rect(294, 170, 436, 118, scale),
        xRadius: 44 * scale,
        yRadius: 44 * scale
    )
    color(0.04, 0.055, 0.070, 0.78).setFill()
    keyDeck.fill()
    color(0.64, 1.00, 0.34, 0.34).setStroke()
    keyDeck.lineWidth = 3 * scale
    keyDeck.stroke()

    for row in 0..<2 {
        for column in 0..<7 {
            let x = 330 + CGFloat(column) * 52 + (row == 1 ? 24 : 0)
            let y = 220 - CGFloat(row) * 40
            let key = NSBezierPath(
                roundedRect: rect(x, y, 34, 18, scale),
                xRadius: 7 * scale,
                yRadius: 7 * scale
            )
            color(0.58, 0.92, 1.00, 0.32).setFill()
            key.fill()
        }
    }

    let cursor = NSBezierPath()
    cursor.move(to: NSPoint(x: 695 * scale, y: 648 * scale))
    cursor.line(to: NSPoint(x: 776 * scale, y: 438 * scale))
    cursor.line(to: NSPoint(x: 704 * scale, y: 462 * scale))
    cursor.line(to: NSPoint(x: 666 * scale, y: 382 * scale))
    cursor.line(to: NSPoint(x: 622 * scale, y: 404 * scale))
    cursor.line(to: NSPoint(x: 659 * scale, y: 484 * scale))
    cursor.line(to: NSPoint(x: 595 * scale, y: 512 * scale))
    cursor.close()
    color(0.96, 1.00, 0.88, 0.96).setFill()
    cursor.fill()
    color(0.64, 1.00, 0.34, 0.72).setStroke()
    cursor.lineWidth = 6 * scale
    cursor.stroke()
}

func drawStatusIcon(scale: CGFloat) {
    let stroke = NSColor.black
    stroke.setStroke()

    let left = NSBezierPath(roundedRect: rect(134, 540, 276, 210, scale), xRadius: 68 * scale, yRadius: 68 * scale)
    left.lineWidth = 76 * scale
    left.stroke()

    let right = NSBezierPath(roundedRect: rect(614, 274, 276, 210, scale), xRadius: 68 * scale, yRadius: 68 * scale)
    right.lineWidth = 76 * scale
    right.stroke()

    let bridge = NSBezierPath()
    bridge.move(to: NSPoint(x: 415 * scale, y: 615 * scale))
    bridge.curve(
        to: NSPoint(x: 608 * scale, y: 382 * scale),
        controlPoint1: NSPoint(x: 535 * scale, y: 630 * scale),
        controlPoint2: NSPoint(x: 505 * scale, y: 370 * scale)
    )
    bridge.lineCapStyle = .round
    bridge.lineWidth = 96 * scale
    bridge.stroke()
}

let iconFiles: [(String, Int)] = [
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

for (name, pixels) in iconFiles {
    let data = try imageData(pixels: pixels, draw: drawAppIcon)
    try data.write(to: iconsetURL.appendingPathComponent(name))
}

let iconutil = Process()
iconutil.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
iconutil.arguments = [
    "-c",
    "icns",
    "-o",
    resourcesURL.appendingPathComponent("DeskBridge.icns").path,
    iconsetURL.path,
]
try iconutil.run()
iconutil.waitUntilExit()
guard iconutil.terminationStatus == 0 else {
    throw NSError(domain: "DeskBridgeIcon", code: Int(iconutil.terminationStatus))
}

try imageData(pixels: 18, draw: drawStatusIcon)
    .write(to: resourcesURL.appendingPathComponent("StatusBarIconTemplate.png"))
try imageData(pixels: 36, draw: drawStatusIcon)
    .write(to: resourcesURL.appendingPathComponent("StatusBarIconTemplate@2x.png"))

try? fileManager.removeItem(at: iconsetURL)
print("Generated DeskBridge macOS icons in \(resourcesURL.path)")

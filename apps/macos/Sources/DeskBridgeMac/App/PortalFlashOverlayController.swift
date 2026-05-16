import AppKit
import QuartzCore

@MainActor
final class PortalFlashOverlayController {
    private var panels: [NSPanel] = []

    func show(_ event: PortalFlashEvent) {
        guard let screen = NSScreen.main else { return }

        let frame = overlayFrame(for: event, screen: screen)
        let panel = NSPanel(
            contentRect: frame,
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        panel.backgroundColor = .clear
        panel.isOpaque = false
        panel.hasShadow = false
        panel.ignoresMouseEvents = true
        panel.level = .statusBar
        panel.collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]
        panel.contentView = PortalFlashView(color: nsColor(for: event.color))
        panel.alphaValue = 1

        panels.append(panel)
        panel.orderFrontRegardless()

        let duration = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
            ? 0.12
            : max(0.14, min(Double(event.durationMs) / 1000.0, 0.8))
        NSAnimationContext.runAnimationGroup { context in
            context.duration = duration
            context.timingFunction = CAMediaTimingFunction(name: .easeOut)
            panel.animator().alphaValue = 0
        } completionHandler: { [weak self, weak panel] in
            Task { @MainActor [weak self, weak panel] in
                guard let panel else { return }
                panel.close()
                self?.panels.removeAll { $0 === panel }
            }
        }
    }

    private func overlayFrame(for event: PortalFlashEvent, screen: NSScreen) -> NSRect {
        let screenFrame = screen.frame
        let thickness: CGFloat = 8
        let span = min(300, max(110, 110 + CGFloat(event.speedPxPerSec) / 14))
        let x = CGFloat(event.x)
        let y = CGFloat(event.y)

        switch event.edge {
        case "right":
            return NSRect(
                x: screenFrame.maxX - thickness,
                y: clamp(screenFrame.maxY - y - span / 2, screenFrame.minY, screenFrame.maxY - span),
                width: thickness,
                height: span
            )
        case "top":
            return NSRect(
                x: clamp(screenFrame.minX + x - span / 2, screenFrame.minX, screenFrame.maxX - span),
                y: screenFrame.maxY - thickness,
                width: span,
                height: thickness
            )
        case "bottom":
            return NSRect(
                x: clamp(screenFrame.minX + x - span / 2, screenFrame.minX, screenFrame.maxX - span),
                y: screenFrame.minY,
                width: span,
                height: thickness
            )
        default:
            return NSRect(
                x: screenFrame.minX,
                y: clamp(screenFrame.maxY - y - span / 2, screenFrame.minY, screenFrame.maxY - span),
                width: thickness,
                height: span
            )
        }
    }

    private func clamp(_ value: CGFloat, _ lower: CGFloat, _ upper: CGFloat) -> CGFloat {
        min(max(value, lower), upper)
    }

    private func nsColor(for id: String) -> NSColor {
        switch id {
        case "aqua":
            NSColor(calibratedRed: 0.35, green: 0.88, blue: 1.00, alpha: 1)
        case "blue":
            NSColor(calibratedRed: 0.38, green: 0.55, blue: 1.00, alpha: 1)
        case "violet":
            NSColor(calibratedRed: 0.78, green: 0.48, blue: 1.00, alpha: 1)
        case "amber":
            NSColor(calibratedRed: 1.00, green: 0.70, blue: 0.25, alpha: 1)
        case "rose":
            NSColor(calibratedRed: 1.00, green: 0.42, blue: 0.56, alpha: 1)
        default:
            NSColor(calibratedRed: 0.70, green: 0.95, blue: 0.48, alpha: 1)
        }
    }
}

private final class PortalFlashView: NSView {
    init(color: NSColor) {
        super.init(frame: .zero)
        wantsLayer = true
        guard let layer else { return }
        layer.backgroundColor = color.withAlphaComponent(0.88).cgColor
        layer.cornerRadius = 4
        layer.shadowColor = color.cgColor
        layer.shadowOpacity = 0.95
        layer.shadowRadius = 18
        layer.shadowOffset = .zero
    }

    required init?(coder: NSCoder) {
        nil
    }
}

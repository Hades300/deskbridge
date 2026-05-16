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
        panel.contentView = PortalFlashView(color: nsColor(for: event.color), edge: event.edge)
        panel.alphaValue = 0

        panels.append(panel)
        panel.orderFrontRegardless()

        let duration = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
            ? 0.12
            : max(0.14, min(Double(event.durationMs) / 1000.0, 0.8))
        let fadeIn = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
            ? 0.01
            : min(0.09, max(0.045, duration * 0.22))
        let fadeOut = max(0.08, duration - fadeIn)
        NSAnimationContext.runAnimationGroup { context in
            context.duration = fadeIn
            context.timingFunction = CAMediaTimingFunction(name: .easeOut)
            panel.animator().alphaValue = 1
        } completionHandler: { [weak self, weak panel] in
            Task { @MainActor [weak self, weak panel] in
                guard let panel else { return }
                NSAnimationContext.runAnimationGroup { context in
                    context.duration = fadeOut
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
        }
    }

    private func overlayFrame(for event: PortalFlashEvent, screen: NSScreen) -> NSRect {
        let screenFrame = screen.frame
        let thickness: CGFloat = 48
        let span = min(380, max(150, 150 + CGFloat(event.speedPxPerSec) / 10))
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
    private let color: NSColor
    private let edge: String
    private let haloLayer = CAGradientLayer()
    private let coreLayer = CALayer()

    init(color: NSColor, edge: String) {
        self.color = color
        self.edge = edge
        super.init(frame: .zero)

        wantsLayer = true
        guard let layer else { return }
        layer.backgroundColor = NSColor.clear.cgColor
        layer.shadowColor = color.cgColor
        layer.shadowOpacity = 0.55
        layer.shadowRadius = 28
        layer.shadowOffset = .zero
        layer.masksToBounds = false

        haloLayer.cornerRadius = 18
        haloLayer.masksToBounds = true
        layer.addSublayer(haloLayer)

        coreLayer.backgroundColor = color.withAlphaComponent(0.92).cgColor
        coreLayer.cornerRadius = 1.5
        coreLayer.shadowColor = color.cgColor
        coreLayer.shadowOpacity = 0.75
        coreLayer.shadowRadius = 18
        coreLayer.shadowOffset = .zero
        layer.addSublayer(coreLayer)
    }

    required init?(coder: NSCoder) {
        nil
    }

    override func layout() {
        super.layout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        haloLayer.frame = bounds
        configureGradient()
        configureCore()
        CATransaction.commit()
    }

    private func configureGradient() {
        let strong = color.withAlphaComponent(0.86).cgColor
        let halo = color.withAlphaComponent(0.58).cgColor
        let mid = color.withAlphaComponent(0.26).cgColor
        let clear = color.withAlphaComponent(0).cgColor

        switch edge {
        case "right":
            haloLayer.startPoint = CGPoint(x: 0, y: 0.5)
            haloLayer.endPoint = CGPoint(x: 1, y: 0.5)
            haloLayer.colors = [clear, mid, halo, strong]
            haloLayer.locations = [0, 0.56, 0.90, 1]
        case "top":
            haloLayer.startPoint = CGPoint(x: 0.5, y: 0)
            haloLayer.endPoint = CGPoint(x: 0.5, y: 1)
            haloLayer.colors = [clear, mid, halo, strong]
            haloLayer.locations = [0, 0.56, 0.90, 1]
        case "bottom":
            haloLayer.startPoint = CGPoint(x: 0.5, y: 0)
            haloLayer.endPoint = CGPoint(x: 0.5, y: 1)
            haloLayer.colors = [strong, halo, mid, clear]
            haloLayer.locations = [0, 0.10, 0.44, 1]
        default:
            haloLayer.startPoint = CGPoint(x: 0, y: 0.5)
            haloLayer.endPoint = CGPoint(x: 1, y: 0.5)
            haloLayer.colors = [strong, halo, mid, clear]
            haloLayer.locations = [0, 0.10, 0.44, 1]
        }
    }

    private func configureCore() {
        let coreThickness: CGFloat = 3
        switch edge {
        case "right":
            coreLayer.frame = CGRect(x: bounds.maxX - coreThickness, y: 0, width: coreThickness, height: bounds.height)
        case "top":
            coreLayer.frame = CGRect(x: 0, y: bounds.maxY - coreThickness, width: bounds.width, height: coreThickness)
        case "bottom":
            coreLayer.frame = CGRect(x: 0, y: 0, width: bounds.width, height: coreThickness)
        default:
            coreLayer.frame = CGRect(x: 0, y: 0, width: coreThickness, height: bounds.height)
        }
    }
}

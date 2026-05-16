import AppKit
import SwiftUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusItem: NSStatusItem?
    private var window: NSWindow?
    private let model = DeskBridgeModel()

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        installStatusItem()
        showWindow()
    }

    private func installStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        item.button?.image = NSImage(systemSymbolName: "keyboard.macwindow", accessibilityDescription: "DeskBridge")
        item.button?.toolTip = "DeskBridge"

        let menu = NSMenu()
        menu.addItem(NSMenuItem(title: "Open DeskBridge", action: #selector(openWindow), keyEquivalent: ""))
        menu.addItem(NSMenuItem(title: "Connect", action: #selector(connect), keyEquivalent: ""))
        menu.addItem(NSMenuItem(title: "Disconnect", action: #selector(disconnect), keyEquivalent: ""))
        menu.addItem(NSMenuItem(title: "Diagnose", action: #selector(diagnose), keyEquivalent: ""))
        menu.addItem(.separator())
        menu.addItem(NSMenuItem(title: "Quit", action: #selector(quit), keyEquivalent: "q"))
        item.menu = menu
        statusItem = item
    }

    private func showWindow() {
        if window == nil {
            let view = DeskBridgeView(model: model)
            let window = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 620, height: 520),
                styleMask: [.titled, .closable, .miniaturizable, .resizable],
                backing: .buffered,
                defer: false
            )
            window.title = "DeskBridge"
            window.contentView = NSHostingView(rootView: view)
            window.minSize = NSSize(width: 560, height: 500)
            window.center()
            self.window = window
        }

        window?.makeKeyAndOrderFront(nil)
        NSApp.activate()
    }

    @objc private func openWindow() {
        showWindow()
    }

    @objc private func connect() {
        model.connect()
    }

    @objc private func disconnect() {
        model.disconnect()
    }

    @objc private func diagnose() {
        showWindow()
        model.runDiagnostics()
    }

    @objc private func quit() {
        model.disconnect()
        NSApp.terminate(nil)
    }
}

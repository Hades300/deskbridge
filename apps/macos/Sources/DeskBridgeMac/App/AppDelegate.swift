import AppKit
import SwiftUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusItem: NSStatusItem?
    private var window: NSWindow?
    private let model = DeskBridgeModel()

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        installStatusItem()
        showWindow()
    }

    func applicationShouldHandleReopen(
        _ sender: NSApplication,
        hasVisibleWindows flag: Bool
    ) -> Bool {
        showWindow()
        return true
    }

    func applicationWillTerminate(_ notification: Notification) {
        model.shutdown()
    }

    private func installStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        item.button?.image = NSImage(systemSymbolName: "keyboard.macwindow", accessibilityDescription: "DeskBridge")
        item.button?.toolTip = "DeskBridge"

        let menu = NSMenu()
        menu.addItem(menuItem("Open DeskBridge", action: #selector(openWindow)))
        menu.addItem(menuItem("Connect", action: #selector(connect)))
        menu.addItem(menuItem("Disconnect", action: #selector(disconnect)))
        menu.addItem(menuItem("Diagnose", action: #selector(diagnose)))
        menu.addItem(.separator())
        menu.addItem(menuItem("Quit", action: #selector(quit), keyEquivalent: "q"))
        item.menu = menu
        statusItem = item
    }

    private func menuItem(
        _ title: String,
        action: Selector,
        keyEquivalent: String = ""
    ) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: keyEquivalent)
        item.target = self
        return item
    }

    private func showWindow() {
        if window == nil {
            let view = DeskBridgeView(model: model)
            let window = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 800, height: 760),
                styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
                backing: .buffered,
                defer: false
            )
            window.title = "DeskBridge"
            window.titleVisibility = .hidden
            window.titlebarAppearsTransparent = true
            window.isMovableByWindowBackground = true
            window.isOpaque = true
            window.backgroundColor = .windowBackgroundColor
            window.appearance = NSAppearance(named: .darkAqua)
            window.contentView = NSHostingView(rootView: view)
            window.minSize = NSSize(width: 860, height: 760)
            window.isReleasedWhenClosed = false
            window.tabbingMode = .disallowed
            window.center()
            self.window = window
        }

        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
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
        model.shutdown()
        NSApp.terminate(nil)
    }
}

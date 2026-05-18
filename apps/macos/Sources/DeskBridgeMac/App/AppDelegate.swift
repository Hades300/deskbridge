import AppKit
import Combine
import SwiftUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuDelegate {
    private var statusItem: NSStatusItem?
    private var statusMenuItem: NSMenuItem?
    private var openMenuItem: NSMenuItem?
    private var connectMenuItem: NSMenuItem?
    private var disconnectMenuItem: NSMenuItem?
    private var diagnosticsMenuItem: NSMenuItem?
    private var window: NSWindow?
    private let model = DeskBridgeModel()
    private var statusObserver: AnyCancellable?

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        installApplicationMenu()
        installStatusItem()
        observeModelStatus()
        showWindow()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
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

    private func installApplicationMenu() {
        let mainMenu = NSMenu()

        let appMenuItem = NSMenuItem()
        mainMenu.addItem(appMenuItem)

        let appMenu = NSMenu(title: "DeskBridge")
        appMenu.addItem(menuItem("About DeskBridge", action: #selector(showAboutPanel)))
        appMenu.addItem(.separator())
        appMenu.addItem(menuItem("Hide DeskBridge", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h", target: NSApp))

        let hideOthersItem = menuItem(
            "Hide Others",
            action: #selector(NSApplication.hideOtherApplications(_:)),
            keyEquivalent: "h",
            target: NSApp
        )
        hideOthersItem.keyEquivalentModifierMask = [.command, .option]
        appMenu.addItem(hideOthersItem)

        appMenu.addItem(menuItem("Show All", action: #selector(NSApplication.unhideAllApplications(_:)), target: NSApp))
        appMenu.addItem(.separator())
        appMenu.addItem(menuItem("Quit DeskBridge", action: #selector(quit), keyEquivalent: "q"))

        appMenuItem.submenu = appMenu
        NSApp.mainMenu = mainMenu
    }

    private func installStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.image = statusBarImage()
        item.button?.imagePosition = .imageOnly
        item.button?.toolTip = "DeskBridge"
        item.button?.appearsDisabled = false

        let menu = NSMenu()
        menu.delegate = self

        let statusLineItem = NSMenuItem(title: "DeskBridge", action: nil, keyEquivalent: "")
        statusLineItem.isEnabled = false
        statusLineItem.image = NSImage(systemSymbolName: "bolt.horizontal.circle", accessibilityDescription: nil)
        menu.addItem(statusLineItem)
        menu.addItem(.separator())

        let openItem = menuItem("Open Control Panel", action: #selector(openWindow))
        openItem.image = NSImage(systemSymbolName: "macwindow", accessibilityDescription: nil)
        menu.addItem(openItem)

        let connectItem = menuItem("Connect", action: #selector(connect))
        connectItem.image = NSImage(systemSymbolName: "play.fill", accessibilityDescription: nil)
        menu.addItem(connectItem)

        let disconnectItem = menuItem("Disconnect", action: #selector(disconnect))
        disconnectItem.image = NSImage(systemSymbolName: "stop.fill", accessibilityDescription: nil)
        menu.addItem(disconnectItem)

        let diagnosticsItem = menuItem("Run Diagnostics", action: #selector(diagnose))
        diagnosticsItem.image = NSImage(systemSymbolName: "waveform.path.ecg", accessibilityDescription: nil)
        menu.addItem(diagnosticsItem)
        menu.addItem(.separator())

        let quitItem = menuItem("Quit DeskBridge", action: #selector(quit), keyEquivalent: "q")
        quitItem.image = NSImage(systemSymbolName: "power", accessibilityDescription: nil)
        menu.addItem(quitItem)

        item.menu = menu

        self.statusMenuItem = statusLineItem
        self.openMenuItem = openItem
        self.connectMenuItem = connectItem
        self.disconnectMenuItem = disconnectItem
        self.diagnosticsMenuItem = diagnosticsItem
        self.statusItem = item
        updateStatusMenu()
    }

    private func menuItem(
        _ title: String,
        action: Selector,
        keyEquivalent: String = "",
        target: AnyObject? = nil
    ) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: keyEquivalent)
        item.target = target ?? self
        return item
    }

    private func statusBarImage() -> NSImage? {
        let image = NSImage(named: "StatusBarIconTemplate")
            ?? NSImage(systemSymbolName: "display.2", accessibilityDescription: "DeskBridge")
        image?.isTemplate = true
        image?.size = NSSize(width: 18, height: 18)
        return image
    }

    private func observeModelStatus() {
        statusObserver = model.$status
            .combineLatest(model.$connected, model.$mode)
            .sink { [weak self] _, _, _ in
                Task { @MainActor in
                    self?.updateStatusMenu()
                }
            }
    }

    func menuNeedsUpdate(_ menu: NSMenu) {
        updateStatusMenu()
    }

    private func updateStatusMenu() {
        let role = model.localRoleLabel
        let status = model.status
        statusMenuItem?.title = "\(role) - \(status)"
        statusItem?.button?.toolTip = "DeskBridge - \(role) - \(status)"

        connectMenuItem?.title = model.mode == .server ? "Start Server" : "Connect"
        disconnectMenuItem?.title = model.mode == .server ? "Stop Server" : "Disconnect"
        connectMenuItem?.isEnabled = !model.connected
        disconnectMenuItem?.isEnabled = model.connected || status != "Idle"
        diagnosticsMenuItem?.isEnabled = true
        openMenuItem?.title = window?.isVisible == true ? "Show Control Panel" : "Open Control Panel"
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

        if window?.isMiniaturized == true {
            window?.deminiaturize(nil)
        }
        window?.makeKeyAndOrderFront(nil)
        window?.orderFrontRegardless()
        NSApp.activate(ignoringOtherApps: true)
        updateStatusMenu()
    }

    @objc private func openWindow() {
        showWindow()
    }

    @objc private func showAboutPanel() {
        var options: [NSApplication.AboutPanelOptionKey: Any] = [
            .applicationName: "DeskBridge",
            .credits: NSAttributedString(string: "Keyboard, mouse, and clipboard sharing for nearby desktops.")
        ]
        if let icon = NSImage(named: "DeskBridge") {
            options[.applicationIcon] = icon
        }
        NSApp.orderFrontStandardAboutPanel(options: options)
        NSApp.activate(ignoringOtherApps: true)
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

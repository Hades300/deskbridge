import AppKit
import ApplicationServices
import Darwin
import Foundation

@MainActor
final class DeskBridgeModel: ObservableObject {
    @Published var server: String
    @Published var screenName: String
    @Published var autoReconnect: Bool
    @Published var status: String = "Idle"
    @Published var connected: Bool = false
    @Published var lastDiagnostics: String = "No diagnostics yet."
    @Published var lastLogLine: String = ""

    private var process: Process?
    private var monitor: Timer?
    private var shouldStayConnected = false
    private var restartScheduled = false
    private let defaults = UserDefaults.standard

    init() {
        server = defaults.string(forKey: "server") ?? "192.168.2.5:24800"
        screenName = defaults.string(forKey: "screenName") ?? "mac"
        autoReconnect = defaults.object(forKey: "autoReconnect") as? Bool ?? true
        startMonitor()
    }

    var normalizedServerAddress: String {
        server.contains(":") ? server : "\(server):24800"
    }

    var configPath: URL {
        supportDirectory.appendingPathComponent("deskbridge.json")
    }

    var binaryPath: String {
        if let override = ProcessInfo.processInfo.environment["DESKBRIDGE_BIN"], !override.isEmpty {
            return override
        }

        let helperExecutable = Bundle.main.bundleURL
            .appendingPathComponent("Contents/Helpers/DeskBridgeHelper.app/Contents/MacOS/deskbridge")
            .path
        if FileManager.default.isExecutableFile(atPath: helperExecutable) {
            return helperExecutable
        }

        let bundledExecutable = Bundle.main.bundleURL
            .appendingPathComponent("Contents/MacOS/deskbridge")
            .path
        if FileManager.default.isExecutableFile(atPath: bundledExecutable) {
            return bundledExecutable
        }

        if let bundled = Bundle.main.resourceURL?.appendingPathComponent("deskbridge").path,
           FileManager.default.isExecutableFile(atPath: bundled) {
            return bundled
        }

        let cwd = FileManager.default.currentDirectoryPath
        let local = URL(fileURLWithPath: cwd)
            .appendingPathComponent("../../target/debug/deskbridge")
            .standardizedFileURL
            .path
        if FileManager.default.isExecutableFile(atPath: local) {
            return local
        }

        return "/usr/local/bin/deskbridge"
    }

    func save() {
        defaults.set(server, forKey: "server")
        defaults.set(screenName, forKey: "screenName")
        defaults.set(autoReconnect, forKey: "autoReconnect")
    }

    func connect() {
        save()
        shouldStayConnected = true
        launchClient()
    }

    func disconnect() {
        shouldStayConnected = false
        restartScheduled = false
        stopClientProcess()
        connected = false
        status = "Idle"
    }

    func shutdown() {
        shouldStayConnected = false
        restartScheduled = false
        monitor?.invalidate()
        monitor = nil
        stopClientProcess()
        connected = false
        status = "Idle"
    }

    func runDiagnostics() {
        save()
        let binary = binaryPath
        let server = normalizedServerAddress
        let name = screenName
        status = connected ? "Connected" : "Diagnosing"

        Task {
            let output = await Task.detached {
                let sections: [(String, [String])] = [
                    ("Local version", ["version"]),
                    ("Reachability", ["diag", "--server", server, "--name", name]),
                    ("Server debug log", ["debug", "--server", server, "--name", name, "server-logs"]),
                    ("Route status", ["debug", "--server", server, "--name", name, "route-status"]),
                    ("Client peer info", ["debug", "--server", server, "--name", name, "peer-info"]),
                    ("Client recent log", ["debug", "--server", server, "--name", name, "logs"]),
                ]

                return sections.map { title, arguments in
                    """
                    \(title)
                    \(String(repeating: "-", count: title.count))
                    \(runDeskBridgeProcess(binary: binary, arguments: arguments))
                    """
                }.joined(separator: "\n\n")
            }.value
            lastDiagnostics = output
            if !connected {
                status = output.contains("protocol: ok") ? "Reachable" : "Needs attention"
            }
        }
    }

    func writeDefaultConfig() {
        let binary = binaryPath
        let path = configPath.path
        createSupportDirectory()
        let output = runDeskBridgeProcess(binary: binary, arguments: ["init-config", "--path", path])
        lastDiagnostics = output
    }

    func openAccessibilitySettings() {
        guard let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") else {
            return
        }
        NSWorkspace.shared.open(url)
    }

    private func launchClient() {
        stopClientProcess()

        guard ensureAccessibilityPermission() else {
            shouldStayConnected = false
            restartScheduled = false
            connected = false
            status = "Accessibility required"
            return
        }

        terminateStaleClientProcesses()

        let process = Process()
        process.executableURL = URL(fileURLWithPath: binaryPath)
        process.arguments = [
            "client",
            "--server", normalizedServerAddress,
            "--name", screenName,
            "--reconnect",
        ]

        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe
        pipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            guard !data.isEmpty, let text = String(data: data, encoding: .utf8) else { return }
            Task { @MainActor [weak self] in
                self?.consumeLog(text)
            }
        }

        process.terminationHandler = { [weak self] terminatedProcess in
            Task { @MainActor [weak self] in
                self?.handleTermination(for: terminatedProcess)
            }
        }

        do {
            try process.run()
            self.process = process
            connected = false
            status = "Connecting"
            markConnectedIfProcessStaysAlive(process)
        } catch {
            connected = false
            status = "Failed to launch"
            lastDiagnostics = error.localizedDescription
            scheduleRestartIfNeeded()
        }
    }

    private func ensureAccessibilityPermission() -> Bool {
        guard mainAppAccessibilityTrusted(prompt: true) else {
            lastDiagnostics = """
            Accessibility permission is required before DeskBridge can inject keyboard and mouse input.

            macOS checks the visible app that launches the helper. Enable DeskBridge in System Settings first; the helper check can only pass after the app is trusted.

            DeskBridge app
            process: \(Bundle.main.bundlePath)
            accessibility: missing

            Helper process
            process: \(binaryPath)

            After granting permission in System Settings, click Connect again.
            """
            openAccessibilitySettings()
            return false
        }

        let output = runDeskBridgeProcess(
            binary: binaryPath,
            arguments: ["permissions", "--prompt"]
        )

        if output.localizedCaseInsensitiveContains("accessibility: ok") {
            return true
        }

        lastDiagnostics = """
        Accessibility permission is required before DeskBridge can inject keyboard and mouse input.

        DeskBridge.app is trusted, but the helper process still failed its permission check. Remove stale DeskBridge entries in System Settings, add DeskBridge again, and enable it.

        \(output)

        After granting permission in System Settings, click Connect again.
        """
        openAccessibilitySettings()
        return false
    }

    private func mainAppAccessibilityTrusted(prompt: Bool) -> Bool {
        let options: CFDictionary = [
            "AXTrustedCheckOptionPrompt": prompt
        ] as CFDictionary
        return AXIsProcessTrustedWithOptions(options)
    }

    private func consumeLog(_ text: String) {
        lastLogLine = text.trimmingCharacters(in: .whitespacesAndNewlines)

        if text.localizedCaseInsensitiveContains("connected") {
            connected = true
            status = "Connected"
        } else if text.localizedCaseInsensitiveContains("failed") {
            connected = false
            status = "Reconnecting"
        } else if text.localizedCaseInsensitiveContains("rejected") {
            connected = false
            status = "Rejected"
        }
    }

    private func handleTermination(for terminatedProcess: Process) {
        guard process === terminatedProcess else { return }

        process = nil
        connected = false

        if shouldStayConnected && autoReconnect {
            status = "Restarting"
            scheduleRestartIfNeeded()
        } else if status != "Idle" {
            status = "Stopped"
        }
    }

    private func scheduleRestartIfNeeded() {
        guard shouldStayConnected, autoReconnect, !restartScheduled else { return }
        restartScheduled = true

        Task { @MainActor in
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            restartScheduled = false
            guard shouldStayConnected, autoReconnect else { return }
            launchClient()
        }
    }

    private func markConnectedIfProcessStaysAlive(_ launchedProcess: Process) {
        Task { @MainActor in
            try? await Task.sleep(nanoseconds: 1_500_000_000)
            guard process === launchedProcess, launchedProcess.isRunning, status == "Connecting" else {
                return
            }
            connected = true
            status = "Connected"
            if lastLogLine.isEmpty {
                lastLogLine = "Client process is running."
            }
        }
    }

    private func startMonitor() {
        monitor = Timer.scheduledTimer(withTimeInterval: 2.0, repeats: true) { [weak self] _ in
            Task { @MainActor [weak self] in
                self?.refreshConnectionState()
            }
        }
    }

    private func refreshConnectionState() {
        guard shouldStayConnected else { return }

        if process == nil || process?.isRunning == false {
            connected = false
            status = autoReconnect ? "Restarting" : "Stopped"
            scheduleRestartIfNeeded()
        }
    }

    private func stopClientProcess() {
        guard let process else { return }

        process.terminationHandler = nil
        if process.isRunning {
            process.terminate()
            process.waitUntilExit()
        }

        self.process = nil
    }

    private func terminateStaleClientProcesses() {
        let matches = runDeskBridgeProcess(
            binary: "/usr/bin/pgrep",
            arguments: ["-f", "\(binaryPath) client"]
        )

        let currentPid = ProcessInfo.processInfo.processIdentifier
        let pids = matches
            .split(whereSeparator: \.isNewline)
            .compactMap { Int32(String($0).trimmingCharacters(in: .whitespacesAndNewlines)) }
            .filter { $0 > 0 && $0 != currentPid }

        guard !pids.isEmpty else { return }

        for pid in pids {
            _ = Darwin.kill(pid, SIGTERM)
        }

        let pidList = pids.map(String.init).joined(separator: ", ")
        lastLogLine = "Stopped stale DeskBridge client process: \(pidList)"
    }

    private var supportDirectory: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("DeskBridge", isDirectory: true)
    }

    private func createSupportDirectory() {
        try? FileManager.default.createDirectory(at: supportDirectory, withIntermediateDirectories: true)
    }
}

private func runDeskBridgeProcess(binary: String, arguments: [String]) -> String {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: binary)
    process.arguments = arguments

    let pipe = Pipe()
    process.standardOutput = pipe
    process.standardError = pipe

    do {
        try process.run()
        process.waitUntilExit()
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        let text = String(data: data, encoding: .utf8) ?? "No output."
        if process.terminationStatus == 0 {
            return text.isEmpty ? "ok" : text
        }
        return text.isEmpty ? "Command exited with \(process.terminationStatus)." : text
    } catch {
        return error.localizedDescription
    }
}

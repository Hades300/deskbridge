import AppKit
import ApplicationServices
import Darwin
import Foundation

enum DeskBridgeMode: String, CaseIterable, Identifiable {
    case client = "Client"
    case server = "Server"

    var id: String { rawValue }
}

enum LayoutPreviewEdge: String {
    case left
    case right
    case top
    case bottom

    var configValue: String {
        switch self {
        case .left: return "left"
        case .right: return "right"
        case .top: return "top"
        case .bottom: return "bottom"
        }
    }
}

/// A DeskBridge server found on the local network via mDNS.
struct DiscoveredServer: Identifiable, Equatable {
    let name: String
    let address: String
    let version: String?

    var id: String { address }
}

/// State of the interactive pairing flow driven by `deskbridge pair`.
enum PairingPhase: Equatable {
    case idle
    case connecting
    case awaitingConfirmation(code: String)
    case succeeded(server: String)
    case failed(message: String)
}

@MainActor
final class DeskBridgeModel: ObservableObject {
    @Published var mode: DeskBridgeMode
    @Published var server: String
    @Published var listenAddress: String
    @Published var screenName: String
    @Published var peerScreenName: String
    @Published var autoReconnect: Bool
    @Published var captureInput: Bool
    @Published var debugLogging: Bool
    @Published var reverseScroll: Bool
    @Published var remoteScrollScale: Double
    @Published var clipboardEnabled: Bool
    @Published var clipboardText: Bool
    @Published var clipboardImage: Bool
    @Published var clipboardFiles: Bool
    @Published var peerOffsetX: Double
    @Published var peerOffsetY: Double
    @Published var status: String = "Idle"
    @Published var connected: Bool = false
    @Published var lastDiagnostics: String = "No diagnostics yet."
    @Published var lastLogLine: String = ""

    @Published var discoveredServers: [DiscoveredServer] = []
    @Published var isDiscovering: Bool = false
    @Published var pairingPhase: PairingPhase = .idle

    let localDisplayWidth: Double
    let localDisplayHeight: Double
    let peerDisplayWidth: Double = 1920
    let peerDisplayHeight: Double = 1080

    private var process: Process?
    private var monitor: Timer?
    private var shouldStayConnected = false
    private var restartScheduled = false
    private var statusProbeInFlight = false
    private var lastStatusProbeAt = Date.distantPast
    private let defaults = UserDefaults.standard
    private let shouldStayConnectedKey = "shouldStayConnected"

    private var pairingProcess: Process?
    private var pairingInput: FileHandle?
    private var pairingBuffer = ""

    init() {
        mode = DeskBridgeMode(rawValue: defaults.string(forKey: "mode") ?? "") ?? .client
        server = defaults.string(forKey: "server") ?? "192.168.2.5:24800"
        listenAddress = defaults.string(forKey: "listenAddress") ?? "0.0.0.0:24800"
        screenName = defaults.string(forKey: "screenName") ?? "mac"
        peerScreenName = defaults.string(forKey: "peerScreenName") ?? "windows"
        autoReconnect = defaults.object(forKey: "autoReconnect") as? Bool ?? true
        captureInput = defaults.object(forKey: "captureInput") as? Bool ?? true
        debugLogging = defaults.object(forKey: "debugLogging") as? Bool ?? false
        reverseScroll = defaults.object(forKey: "reverseScroll") as? Bool ?? false
        remoteScrollScale = Self.normalizeRemoteScrollScale(defaults.object(forKey: "remoteScrollScale") as? Double ?? 1.0)
        clipboardEnabled = defaults.object(forKey: "clipboardEnabled") as? Bool ?? true
        clipboardText = defaults.object(forKey: "clipboardText") as? Bool ?? true
        clipboardImage = defaults.object(forKey: "clipboardImage") as? Bool ?? true
        clipboardFiles = defaults.object(forKey: "clipboardFiles") as? Bool ?? true
        peerOffsetX = defaults.object(forKey: "peerOffsetX") as? Double ?? -1920
        peerOffsetY = defaults.object(forKey: "peerOffsetY") as? Double ?? 0
        shouldStayConnected = defaults.object(forKey: shouldStayConnectedKey) as? Bool ?? false

        let screenFrame = NSScreen.main?.frame ?? NSRect(x: 0, y: 0, width: 1728, height: 1117)
        localDisplayWidth = max(1, screenFrame.width.rounded())
        localDisplayHeight = max(1, screenFrame.height.rounded())

        startMonitor()

        if shouldStayConnected {
            status = mode == .client ? "Connecting" : "Starting"
            Task { @MainActor [weak self] in
                try? await Task.sleep(nanoseconds: 300_000_000)
                guard let self, self.shouldStayConnected else { return }
                self.launchForCurrentMode()
            }
        }
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

    var localRoleLabel: String {
        mode == .server ? "Server" : "Client"
    }

    var peerRoleLabel: String {
        mode == .server ? "Client" : "Server"
    }

    var localToPeerEdge: LayoutPreviewEdge {
        edge(
            fromOriginX: 0,
            fromOriginY: 0,
            fromWidth: localDisplayWidth,
            fromHeight: localDisplayHeight,
            toOriginX: peerOffsetX,
            toOriginY: peerOffsetY,
            toWidth: peerDisplayWidth,
            toHeight: peerDisplayHeight
        )
    }

    var entryDescription: String {
        let edgeFromServer = serverLinkEdge()
        let serverName = mode == .server ? screenName : peerScreenName
        let clientName = mode == .server ? peerScreenName : screenName
        return "\(serverName) \(edgeFromServer.rawValue.capitalized) -> \(clientName)"
    }

    func save() {
        defaults.set(mode.rawValue, forKey: "mode")
        defaults.set(server, forKey: "server")
        defaults.set(listenAddress, forKey: "listenAddress")
        defaults.set(screenName, forKey: "screenName")
        defaults.set(peerScreenName, forKey: "peerScreenName")
        defaults.set(autoReconnect, forKey: "autoReconnect")
        defaults.set(captureInput, forKey: "captureInput")
        defaults.set(debugLogging, forKey: "debugLogging")
        defaults.set(reverseScroll, forKey: "reverseScroll")
        defaults.set(Self.normalizeRemoteScrollScale(remoteScrollScale), forKey: "remoteScrollScale")
        defaults.set(clipboardEnabled, forKey: "clipboardEnabled")
        defaults.set(clipboardText, forKey: "clipboardText")
        defaults.set(clipboardImage, forKey: "clipboardImage")
        defaults.set(clipboardFiles, forKey: "clipboardFiles")
        defaults.set(peerOffsetX, forKey: "peerOffsetX")
        defaults.set(peerOffsetY, forKey: "peerOffsetY")
    }

    func connect() {
        save()
        shouldStayConnected = true
        defaults.set(true, forKey: shouldStayConnectedKey)
        launchForCurrentMode()
    }

    func disconnect() {
        shouldStayConnected = false
        defaults.set(false, forKey: shouldStayConnectedKey)
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

    func snapPeerToNearestEdge() {
        let localCenterX = localDisplayWidth / 2
        let localCenterY = localDisplayHeight / 2
        let peerCenterX = peerOffsetX + peerDisplayWidth / 2
        let peerCenterY = peerOffsetY + peerDisplayHeight / 2
        let dx = peerCenterX - localCenterX
        let dy = peerCenterY - localCenterY
        let minOverlap = 140.0

        if abs(dx / localDisplayWidth) >= abs(dy / localDisplayHeight) {
            peerOffsetX = dx >= 0 ? localDisplayWidth : -peerDisplayWidth
            peerOffsetY = peerOffsetY.clamped(to: (-peerDisplayHeight + minOverlap)...(localDisplayHeight - minOverlap))
        } else {
            peerOffsetY = dy >= 0 ? localDisplayHeight : -peerDisplayHeight
            peerOffsetX = peerOffsetX.clamped(to: (-peerDisplayWidth + minOverlap)...(localDisplayWidth - minOverlap))
        }
        applyRuntimeInputSettings()
    }

    func runDiagnostics() {
        save()
        let binary = binaryPath
        let server = mode == .client ? normalizedServerAddress : localDebugServerAddress
        let name = mode == .server ? peerScreenName : screenName
        status = connected ? currentConnectedStatus : "Diagnosing"

        Task {
            let output = await Task.detached {
                let sections: [(String, [String])] = [
                    ("Local version", ["version"]),
                    ("Reachability", ["diag", "--server", server, "--name", name]),
                    ("Runtime input settings", ["debug", "--server", server, "--name", name, "input-settings"]),
                    ("Server debug log", ["debug", "--server", server, "--name", name, "server-logs"]),
                    ("Route status", ["debug", "--server", server, "--name", name, "route-status"]),
                    ("Performance", ["debug", "--server", server, "--name", name, "perf"]),
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
        save()
        do {
            try writeGeneratedConfig()
            lastDiagnostics = "Wrote config:\n\(configPath.path)\n\n\(entryDescription)"
        } catch {
            lastDiagnostics = error.localizedDescription
        }
    }

    func applyRuntimeInputSettings() {
        save()

        guard mode == .server else {
            return
        }

        do {
            try writeGeneratedConfig()
        } catch {
            lastDiagnostics = error.localizedDescription
            return
        }

        guard process?.isRunning == true else {
            return
        }

        let binary = binaryPath
        let config = configPath.path
        let server = localDebugServerAddress
        let name = peerScreenName
        let reverseValue = reverseScroll ? "true" : "false"
        let remoteScrollScaleValue = String(
            format: "%.3f",
            locale: Locale(identifier: "en_US_POSIX"),
            Self.normalizeRemoteScrollScale(remoteScrollScale)
        )

        Task {
            let output = await Task.detached {
                runDeskBridgeProcess(
                    binary: binary,
                    arguments: [
                        "debug",
                        "--config", config,
                        "--server", server,
                        "--name", name,
                        "input-settings",
                        "--apply-config",
                        "--reverse-scroll", reverseValue,
                        "--remote-scroll-scale", remoteScrollScaleValue,
                    ]
                )
            }.value
            lastDiagnostics = "Applied runtime input settings:\n\(output)"
        }
    }

    func openAccessibilitySettings() {
        guard let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") else {
            return
        }
        NSWorkspace.shared.open(url)
    }

    // MARK: - Discovery and pairing

    /// Browse the local network for servers advertised over mDNS.
    func discoverServers() {
        guard !isDiscovering else { return }
        isDiscovering = true
        discoveredServers = []
        let binary = binaryPath

        Task { @MainActor in
            let output = await Task.detached {
                runDeskBridgeProcess(binary: binary, arguments: ["discover", "--timeout-ms", "2500"])
            }.value
            self.discoveredServers = Self.parseDiscoverOutput(output)
            self.isDiscovering = false
        }
    }

    static func parseDiscoverOutput(_ output: String) -> [DiscoveredServer] {
        var servers: [DiscoveredServer] = []
        for raw in output.split(whereSeparator: \.isNewline) {
            let line = String(raw).trimmingCharacters(in: .whitespaces)
            let parts = line.split(separator: "\t").map(String.init)
            guard parts.count >= 2, parts[1].contains(":") else { continue }
            let version = parts.count >= 3 ? String(parts[2]) : nil
            servers.append(DiscoveredServer(name: parts[0], address: parts[1], version: version))
        }
        return servers
    }

    /// Start interactive pairing as the joining device. The host shows a code
    /// too; the user confirms the codes match before the secret is exchanged.
    func startPairing(with address: String) {
        cancelPairing()
        let normalized = address.contains(":") ? address : "\(address):24800"
        createSupportDirectory()
        pairingBuffer = ""
        pairingPhase = .connecting

        let process = Process()
        process.executableURL = URL(fileURLWithPath: binaryPath)
        process.arguments = ["pair", "--join", normalized, "--config", configPath.path]

        let outPipe = Pipe()
        let inPipe = Pipe()
        process.standardOutput = outPipe
        process.standardError = outPipe
        process.standardInput = inPipe
        pairingInput = inPipe.fileHandleForWriting

        outPipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            guard !data.isEmpty, let text = String(data: data, encoding: .utf8) else { return }
            Task { @MainActor [weak self] in
                self?.consumePairingOutput(text, server: normalized)
            }
        }

        process.terminationHandler = { [weak self] finished in
            Task { @MainActor [weak self] in
                self?.handlePairingTermination(finished, server: normalized)
            }
        }

        do {
            try process.run()
            pairingProcess = process
        } catch {
            pairingPhase = .failed(message: error.localizedDescription)
            pairingInput = nil
        }
    }

    /// The user confirmed the codes match: let the host send the secret.
    func confirmPairing() {
        guard case .awaitingConfirmation = pairingPhase, let input = pairingInput else { return }
        try? input.write(contentsOf: Data("y\n".utf8))
    }

    /// Abort an in-progress pairing (or dismiss a finished one).
    func cancelPairing() {
        if let input = pairingInput {
            try? input.write(contentsOf: Data("n\n".utf8))
        }
        pairingProcess?.terminationHandler = nil
        if pairingProcess?.isRunning == true {
            pairingProcess?.terminate()
        }
        pairingProcess = nil
        pairingInput = nil
        pairingBuffer = ""
        if case .succeeded = pairingPhase {
            // Keep the success state visible until the sheet is dismissed.
        } else {
            pairingPhase = .idle
        }
    }

    private func consumePairingOutput(_ text: String, server: String) {
        pairingBuffer += text

        if case .awaitingConfirmation = pairingPhase {
            // Already showing the code; just watch for completion below.
        } else if let code = Self.extractPairingCode(pairingBuffer) {
            pairingPhase = .awaitingConfirmation(code: code)
        }

        if pairingBuffer.localizedCaseInsensitiveContains("Paired.") {
            adoptPairedServer(server)
        }
    }

    private func handlePairingTermination(_ finished: Process, server: String) {
        guard pairingProcess === finished else { return }
        pairingProcess = nil
        pairingInput = nil

        if pairingBuffer.localizedCaseInsensitiveContains("Paired.") {
            adoptPairedServer(server)
        } else if case .succeeded = pairingPhase {
            // Already succeeded.
        } else {
            let reason = Self.lastMeaningfulLine(pairingBuffer) ?? "Pairing did not complete."
            pairingPhase = .failed(message: reason)
        }
    }

    private func adoptPairedServer(_ server: String) {
        pairingPhase = .succeeded(server: server)
        self.server = server
        mode = .client
        save()
    }

    static func extractPairingCode(_ buffer: String) -> String? {
        for raw in buffer.split(whereSeparator: \.isNewline) {
            let line = String(raw)
            guard let range = line.range(of: "Pairing code:") else { continue }
            let code = line[range.upperBound...].trimmingCharacters(in: .whitespaces)
            if !code.isEmpty {
                return code
            }
        }
        return nil
    }

    static func lastMeaningfulLine(_ buffer: String) -> String? {
        buffer
            .split(whereSeparator: \.isNewline)
            .map { String($0).trimmingCharacters(in: .whitespaces) }
            .last { !$0.isEmpty }
    }

    private var currentConnectedStatus: String {
        mode == .server ? "Server running" : "Connected"
    }

    private var localDebugServerAddress: String {
        "127.0.0.1:\(listenPort(from: listenAddress))"
    }

    private func launchForCurrentMode() {
        switch mode {
        case .client:
            launchClient()
        case .server:
            launchServer()
        }
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

        terminateStaleDaemonProcesses(argumentsContain: " client")

        var arguments = [
            "client",
            "--server", normalizedServerAddress,
            "--name", screenName,
            "--reconnect",
        ]
        if let secret = configuredSecret() {
            arguments.append(contentsOf: ["--psk", secret])
        }

        launchDaemon(arguments: arguments, launchingStatus: "Connecting")
    }

    private func launchServer() {
        stopClientProcess()

        guard ensureAccessibilityPermission() else {
            shouldStayConnected = false
            restartScheduled = false
            connected = false
            status = "Accessibility required"
            return
        }

        do {
            try writeGeneratedConfig()
        } catch {
            connected = false
            status = "Config failed"
            lastDiagnostics = error.localizedDescription
            return
        }

        terminateStaleDaemonProcesses(argumentsContain: " server")

        var arguments = ["server", "--config", configPath.path]
        if captureInput {
            arguments.append("--capture")
        }
        if debugLogging {
            arguments.append("--debug-capture-log")
        }
        if reverseScroll {
            arguments.append("--reverse-scroll")
        }
        arguments.append(contentsOf: [
            "--remote-scroll-scale",
            String(
                format: "%.3f",
                locale: Locale(identifier: "en_US_POSIX"),
                Self.normalizeRemoteScrollScale(remoteScrollScale)
            ),
        ])

        launchDaemon(arguments: arguments, launchingStatus: "Starting")
    }

    private func launchDaemon(arguments: [String], launchingStatus: String) {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: binaryPath)
        process.arguments = arguments

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
            status = launchingStatus
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
        let lines = text
            .split(whereSeparator: \.isNewline)
            .map { String($0).trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }

        for line in lines {
            lastLogLine = line

            if line.localizedCaseInsensitiveContains("connected") {
                connected = true
                status = currentConnectedStatus
            } else if line.localizedCaseInsensitiveContains("server listening") {
                connected = true
                status = currentConnectedStatus
            } else if line.localizedCaseInsensitiveContains("failed") {
                connected = false
                status = mode == .client ? "Reconnecting" : "Restarting"
            } else if line.localizedCaseInsensitiveContains("rejected") {
                connected = false
                status = "Rejected"
            }
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
            launchForCurrentMode()
        }
    }

    private func markConnectedIfProcessStaysAlive(_ launchedProcess: Process) {
        Task { @MainActor in
            try? await Task.sleep(nanoseconds: 1_500_000_000)
            guard process === launchedProcess, launchedProcess.isRunning else {
                return
            }

            if mode == .server {
                connected = true
                status = currentConnectedStatus
                if lastLogLine.isEmpty {
                    lastLogLine = "Server process is running."
                }
            } else {
                probeConnectionState(force: true)
                if status == "Connecting" {
                    status = "Verifying"
                }
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
            return
        }

        if mode == .client {
            probeConnectionState(force: false)
        } else {
            connected = true
            status = currentConnectedStatus
        }
    }

    private func probeConnectionState(force: Bool) {
        guard mode == .client, !statusProbeInFlight else { return }
        if !force && Date().timeIntervalSince(lastStatusProbeAt) < 3 {
            return
        }
        lastStatusProbeAt = Date()
        statusProbeInFlight = true

        let binary = binaryPath
        let server = normalizedServerAddress
        let name = screenName
        Task {
            let output = await Task.detached {
                runDeskBridgeProcess(binary: binary, arguments: ["debug", "--server", server, "--name", name, "peer-info"])
            }.value

            statusProbeInFlight = false
            if output.localizedCaseInsensitiveContains("client peer info read") || output.localizedCaseInsensitiveContains("role=client") {
                connected = true
                status = currentConnectedStatus
                lastLogLine = "Connection confirmed by server."
            } else if shouldStayConnected {
                connected = false
                status = "Reconnecting"
                if !output.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    lastLogLine = output.trimmingCharacters(in: .whitespacesAndNewlines)
                }
            }
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

    private func terminateStaleDaemonProcesses(argumentsContain marker: String) {
        let matches = runDeskBridgeProcess(
            binary: "/usr/bin/pgrep",
            arguments: ["-f", "\(binaryPath)\(marker)"]
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
        lastLogLine = "Stopped stale DeskBridge daemon process: \(pidList)"
    }

    /// Read the pairing secret persisted in the config, if any.
    func configuredSecret() -> String? {
        guard let data = try? Data(contentsOf: configPath),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let security = object["security"] as? [String: Any],
              let psk = security["psk"] as? String,
              !psk.isEmpty
        else {
            return nil
        }
        return psk
    }

    private func writeGeneratedConfig() throws {
        createSupportDirectory()

        let serverScreenName = mode == .server ? screenName : peerScreenName
        let clientScreenName = mode == .server ? peerScreenName : screenName
        let serverOrigin = mode == .server
            ? ["x": 0, "y": 0]
            : ["x": Int(peerOffsetX.rounded()), "y": Int(peerOffsetY.rounded())]
        let clientOrigin = mode == .server
            ? ["x": Int(peerOffsetX.rounded()), "y": Int(peerOffsetY.rounded())]
            : ["x": 0, "y": 0]
        let serverSize = mode == .server
            ? ["width": Int(localDisplayWidth), "height": Int(localDisplayHeight)]
            : ["width": Int(peerDisplayWidth), "height": Int(peerDisplayHeight)]
        let clientSize = mode == .server
            ? ["width": Int(peerDisplayWidth), "height": Int(peerDisplayHeight)]
            : ["width": Int(localDisplayWidth), "height": Int(localDisplayHeight)]

        // Preserve a paired secret across config regenerations.
        let existingSecret = configuredSecret()

        var config: [String: Any] = [
            "server": [
                "name": serverScreenName,
                "listen": listenAddress,
            ],
            "client": [
                "name": clientScreenName,
                "server_addr": normalizedServerAddress,
            ],
            "layout": [
                "screens": [
                    [
                        "name": serverScreenName,
                        "size": serverSize,
                        "origin": serverOrigin,
                    ],
                    [
                        "name": clientScreenName,
                        "size": clientSize,
                        "origin": clientOrigin,
                    ],
                ],
                "links": [
                    [
                        "from": serverScreenName,
                        "edge": serverLinkEdge().configValue,
                        "to": clientScreenName,
                    ],
                ],
            ],
            "reliability": [
                "heartbeat_ms": 2000,
                "reconnect_max_ms": 10000,
                "stale_after_ms": 6000,
            ],
            "input": [
                "reverse_scroll": mode == .server && reverseScroll,
                "remote_scroll_scale": mode == .server ? Self.normalizeRemoteScrollScale(remoteScrollScale) : 1.0,
            ],
            "clipboard": [
                "enabled": clipboardEnabled,
                "text": clipboardText,
                "image": clipboardImage,
                "files": clipboardFiles,
                "poll_ms": 750,
                "max_transfer_bytes": 33_554_432,
            ],
        ]

        if let secret = existingSecret {
            config["security"] = ["psk": secret]
        }

        let data = try JSONSerialization.data(withJSONObject: config, options: [.prettyPrinted, .sortedKeys])
        try data.write(to: configPath, options: .atomic)
    }

    private static func normalizeRemoteScrollScale(_ value: Double) -> Double {
        guard value.isFinite else {
            return 1.0
        }

        return min(2.0, max(0.25, (value * 100).rounded() / 100))
    }

    private func serverLinkEdge() -> LayoutPreviewEdge {
        if mode == .server {
            return edge(
                fromOriginX: 0,
                fromOriginY: 0,
                fromWidth: localDisplayWidth,
                fromHeight: localDisplayHeight,
                toOriginX: peerOffsetX,
                toOriginY: peerOffsetY,
                toWidth: peerDisplayWidth,
                toHeight: peerDisplayHeight
            )
        }

        return edge(
            fromOriginX: peerOffsetX,
            fromOriginY: peerOffsetY,
            fromWidth: peerDisplayWidth,
            fromHeight: peerDisplayHeight,
            toOriginX: 0,
            toOriginY: 0,
            toWidth: localDisplayWidth,
            toHeight: localDisplayHeight
        )
    }

    private func edge(
        fromOriginX: Double,
        fromOriginY: Double,
        fromWidth: Double,
        fromHeight: Double,
        toOriginX: Double,
        toOriginY: Double,
        toWidth: Double,
        toHeight: Double
    ) -> LayoutPreviewEdge {
        let fromCenterX = fromOriginX + fromWidth / 2
        let fromCenterY = fromOriginY + fromHeight / 2
        let toCenterX = toOriginX + toWidth / 2
        let toCenterY = toOriginY + toHeight / 2
        let dx = toCenterX - fromCenterX
        let dy = toCenterY - fromCenterY

        if abs(dx / max(fromWidth, 1)) >= abs(dy / max(fromHeight, 1)) {
            return dx >= 0 ? .right : .left
        }
        return dy >= 0 ? .bottom : .top
    }

    private func listenPort(from address: String) -> String {
        guard let colon = address.lastIndex(of: ":") else {
            return "24800"
        }
        let port = address[address.index(after: colon)...]
        return port.isEmpty ? "24800" : String(port)
    }

    private var supportDirectory: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("DeskBridge", isDirectory: true)
    }

    private func createSupportDirectory() {
        try? FileManager.default.createDirectory(at: supportDirectory, withIntermediateDirectories: true)
    }
}

private extension Comparable {
    func clamped(to range: ClosedRange<Self>) -> Self {
        min(max(self, range.lowerBound), range.upperBound)
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

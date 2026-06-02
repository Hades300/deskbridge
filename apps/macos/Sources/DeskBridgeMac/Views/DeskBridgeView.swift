import AppKit
import SwiftUI

struct DeskBridgeView: View {
    @ObservedObject var model: DeskBridgeModel
    @State private var dragStartOffset: CGSize?
    @State private var dragStartScale: Double = 0.08
    @State private var diagnosticsExpanded = false
    @State private var showPairing = false

    var body: some View {
        ZStack {
            DeskBridgeTheme.windowBackground
                .ignoresSafeArea()

            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    header
                    controlBar
                    connectionPanel
                    if model.mode == .server {
                        layoutEditor
                    }
                    statusPanel
                    diagnosticsPanel
                }
                .padding(.horizontal, 24)
                .padding(.top, 20)
                .padding(.bottom, 24)
            }
        }
        .frame(minWidth: 860, idealWidth: 920, minHeight: 760)
        .sheet(isPresented: $showPairing) {
            PairingView(model: model, isPresented: $showPairing)
        }
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 16) {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 10) {
                    Image(systemName: "keyboard.macwindow")
                        .font(.system(size: 20, weight: .semibold))
                        .symbolRenderingMode(.hierarchical)
                        .foregroundStyle(DeskBridgeTheme.accent)

                    Text("DeskBridge")
                        .font(.system(size: 28, weight: .semibold))
                        .tracking(-0.2)
                }

                HStack(spacing: 8) {
                    statusPill
                    infoPill(model.mode.rawValue, systemImage: model.mode == .server ? "antenna.radiowaves.left.and.right" : "bolt.horizontal")
                    infoPill(model.clipboardEnabled ? "Clipboard on" : "Clipboard off", systemImage: "doc.on.clipboard")
                }
            }

            Spacer(minLength: 16)

            modePicker
                .frame(width: 190)
        }
    }

    private var statusPill: some View {
        HStack(spacing: 7) {
            Circle()
                .fill(model.connected ? DeskBridgeTheme.success : DeskBridgeTheme.warning)
                .frame(width: 8, height: 8)
                .shadow(color: (model.connected ? DeskBridgeTheme.success : DeskBridgeTheme.warning).opacity(0.55), radius: 6)

            Text(model.connected ? "Connected" : model.status)
                .font(.system(size: 12, weight: .medium))
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(DeskBridgeTheme.pillBackground, in: Capsule())
        .overlay(Capsule().stroke(DeskBridgeTheme.hairline))
    }

    private func infoPill(_ text: String, systemImage: String) -> some View {
        HStack(spacing: 6) {
            Image(systemName: systemImage)
                .font(.system(size: 11, weight: .medium))
            Text(text)
                .font(.system(size: 12, weight: .medium))
        }
        .foregroundStyle(.secondary)
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(DeskBridgeTheme.pillBackground, in: Capsule())
        .overlay(Capsule().stroke(DeskBridgeTheme.hairline))
    }

    private var modePicker: some View {
        Picker("Mode", selection: $model.mode) {
            ForEach(DeskBridgeMode.allCases) { mode in
                Text(mode.rawValue).tag(mode)
            }
        }
        .pickerStyle(.segmented)
        .onChange(of: model.mode) { _, _ in
            model.save()
        }
    }

    private var controlBar: some View {
        HStack(spacing: 10) {
            Button {
                model.connect()
            } label: {
                Label(model.mode == .server ? "Start" : "Connect", systemImage: "play.fill")
            }
            .keyboardShortcut(.defaultAction)
            .buttonStyle(.borderedProminent)
            .tint(DeskBridgeTheme.accent)

            Button {
                model.disconnect()
            } label: {
                Label(model.mode == .server ? "Stop" : "Disconnect", systemImage: "stop.fill")
            }

            if model.mode == .client {
                Button {
                    model.pairingPhase = .idle
                    showPairing = true
                } label: {
                    Label("Pair", systemImage: "qrcode")
                }
            }

            Spacer(minLength: 14)

            Button {
                model.openAccessibilitySettings()
            } label: {
                Label("Accessibility", systemImage: "lock.shield")
            }

            Button {
                model.runDiagnostics()
            } label: {
                Label("Diagnose", systemImage: "waveform.path.ecg")
            }

            if model.mode == .server {
                Button {
                    model.writeDefaultConfig()
                } label: {
                    Label("Save Config", systemImage: "square.and.arrow.down")
                }
            }
        }
        .buttonStyle(.bordered)
    }

    private var connectionPanel: some View {
        sectionSurface {
            VStack(alignment: .leading, spacing: 14) {
                sectionHeader(
                    title: "Connection",
                    subtitle: model.mode == .server ? model.listenAddress : model.normalizedServerAddress,
                    systemImage: "point.3.connected.trianglepath.dotted"
                )

                Grid(alignment: .leading, horizontalSpacing: 18, verticalSpacing: 12) {
                    if model.mode == .client {
                        GridRow {
                            settingLabel("Server", systemImage: "desktopcomputer")
                            styledTextField("192.168.2.5:24800", text: $model.server)
                        }
                    } else {
                        GridRow {
                            settingLabel("Listen", systemImage: "network")
                            styledTextField("0.0.0.0:24800", text: $model.listenAddress)
                        }
                    }

                    GridRow {
                        settingLabel("Local", systemImage: model.mode == .server ? "keyboard" : "macwindow")
                        styledTextField("mac", text: $model.screenName)
                    }

                    GridRow {
                        settingLabel("Peer", systemImage: "rectangle.connected.to.line.below")
                        styledTextField("windows", text: $model.peerScreenName)
                    }

                    GridRow {
                        settingLabel("Recovery", systemImage: "arrow.clockwise")
                        HStack(spacing: 18) {
                            Toggle("Auto reconnect", isOn: $model.autoReconnect)
                                .toggleStyle(.checkbox)
                            if model.mode == .server {
                                Toggle("Reverse remote wheel", isOn: $model.reverseScroll)
                                    .toggleStyle(.checkbox)
                            }
                        }
                        .onChange(of: model.autoReconnect) { _, _ in model.save() }
                        .onChange(of: model.reverseScroll) { _, _ in model.applyRuntimeInputSettings() }
                    }

                    if model.mode == .server {
                        GridRow {
                            settingLabel("Wheel", systemImage: "scroll")
                            HStack(spacing: 10) {
                                Text("Remote speed")
                                    .foregroundStyle(.secondary)
                                Slider(value: $model.remoteScrollScale, in: 0.25...2.0, step: 0.05)
                                    .frame(width: 150)
                                Text("\(model.remoteScrollScale, specifier: "%.2f")x")
                                    .font(.system(size: 12, weight: .medium, design: .monospaced))
                                    .foregroundStyle(.secondary)
                                    .frame(width: 48, alignment: .trailing)
                            }
                            .onChange(of: model.remoteScrollScale) { _, _ in model.applyRuntimeInputSettings() }
                        }
                    }

                    GridRow {
                        settingLabel("Clipboard", systemImage: "doc.on.clipboard")
                        HStack(spacing: 14) {
                            Toggle("Sync", isOn: $model.clipboardEnabled)
                                .toggleStyle(.checkbox)
                            Toggle("Text", isOn: $model.clipboardText)
                                .toggleStyle(.checkbox)
                            Toggle("Image", isOn: $model.clipboardImage)
                                .toggleStyle(.checkbox)
                            Toggle("Files", isOn: $model.clipboardFiles)
                                .toggleStyle(.checkbox)
                        }
                        .onChange(of: model.clipboardEnabled) { _, _ in model.save() }
                        .onChange(of: model.clipboardText) { _, _ in model.save() }
                        .onChange(of: model.clipboardImage) { _, _ in model.save() }
                        .onChange(of: model.clipboardFiles) { _, _ in model.save() }
                    }

                    if model.mode == .server {
                        GridRow {
                            settingLabel("Capture", systemImage: "cursorarrow.motionlines")
                            HStack(spacing: 18) {
                                Toggle("Keyboard and mouse", isOn: $model.captureInput)
                                    .toggleStyle(.checkbox)
                                Toggle("Capture debug log", isOn: $model.debugLogging)
                                    .toggleStyle(.checkbox)
                            }
                            .onChange(of: model.captureInput) { _, _ in model.save() }
                            .onChange(of: model.debugLogging) { _, _ in model.save() }
                        }
                    }
                }
            }
        }
    }

    private var layoutEditor: some View {
        sectionSurface {
            VStack(alignment: .leading, spacing: 12) {
                HStack(alignment: .center) {
                    sectionHeader(
                        title: "Display Layout",
                        subtitle: model.entryDescription,
                        systemImage: "rectangle.3.group"
                    )

                    Spacer()

                    Button {
                        model.snapPeerToNearestEdge()
                    } label: {
                        Label("Snap", systemImage: "rectangle.2.swap")
                    }
                    .buttonStyle(.bordered)
                }

                GeometryReader { proxy in
                    let metrics = layoutMetrics(for: proxy.size)

                    ZStack(alignment: .topLeading) {
                        RoundedRectangle(cornerRadius: 8)
                            .fill(DeskBridgeTheme.canvasBackground)
                            .overlay(RoundedRectangle(cornerRadius: 8).stroke(DeskBridgeTheme.hairline))

                        gridLines

                        screenBox(
                            title: model.screenName,
                            subtitle: "\(model.localRoleLabel) - \(Int(model.localDisplayWidth))x\(Int(model.localDisplayHeight))",
                            size: metrics.localSize,
                            isLocal: true
                        )
                        .position(
                            x: metrics.localOrigin.x + metrics.localSize.width / 2,
                            y: metrics.localOrigin.y + metrics.localSize.height / 2
                        )

                        screenBox(
                            title: model.peerScreenName,
                            subtitle: "\(model.peerRoleLabel) - \(Int(model.peerDisplayWidth))x\(Int(model.peerDisplayHeight))",
                            size: metrics.peerSize,
                            isLocal: false
                        )
                        .position(
                            x: metrics.peerOrigin.x + metrics.peerSize.width / 2,
                            y: metrics.peerOrigin.y + metrics.peerSize.height / 2
                        )
                        .gesture(screenDragGesture(scale: metrics.scale))
                    }
                }
                .frame(height: 220)
            }
        }
    }

    private var gridLines: some View {
        Canvas { context, size in
            var path = Path()
            let step: CGFloat = 24
            var x: CGFloat = 0
            while x <= size.width {
                path.move(to: CGPoint(x: x, y: 0))
                path.addLine(to: CGPoint(x: x, y: size.height))
                x += step
            }
            var y: CGFloat = 0
            while y <= size.height {
                path.move(to: CGPoint(x: 0, y: y))
                path.addLine(to: CGPoint(x: size.width, y: y))
                y += step
            }
            context.stroke(path, with: .color(.secondary.opacity(0.10)), lineWidth: 1)
        }
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }

    private func screenBox(
        title: String,
        subtitle: String,
        size: CGSize,
        isLocal: Bool
    ) -> some View {
        let tint = isLocal ? DeskBridgeTheme.localScreen : DeskBridgeTheme.peerScreen
        return ZStack {
            RoundedRectangle(cornerRadius: 7)
                .fill(tint.opacity(0.16))
                .overlay(
                    RoundedRectangle(cornerRadius: 7)
                        .stroke(tint.opacity(0.58), lineWidth: 1)
                )

            VStack(spacing: 4) {
                Text(title)
                    .font(.system(size: 14, weight: .semibold))
                    .lineLimit(1)
                Text(subtitle)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            .padding(.horizontal, 10)
        }
        .frame(width: size.width, height: size.height)
        .shadow(color: tint.opacity(0.16), radius: 12, y: 5)
    }

    private var statusPanel: some View {
        sectionSurface {
            VStack(alignment: .leading, spacing: 12) {
                sectionHeader(title: "Runtime", subtitle: model.status, systemImage: "gauge.with.dots.needle.50percent")

                Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 9) {
                    GridRow {
                        detailLabel(model.mode == .server ? "Listen" : "Target")
                        detailValue(model.mode == .server ? model.listenAddress : model.normalizedServerAddress)
                    }
                    GridRow {
                        detailLabel("Binary")
                        detailValue(model.binaryPath, lineLimit: 1)
                    }
                    GridRow {
                        detailLabel("Last log")
                        detailValue(model.lastLogLine.isEmpty ? "No client log yet." : model.lastLogLine, lineLimit: 2)
                    }
                }
            }
        }
    }

    private var diagnosticsPanel: some View {
        sectionSurface {
            DisclosureGroup(isExpanded: $diagnosticsExpanded) {
                ScrollView {
                    Text(model.lastDiagnostics)
                        .font(.system(size: 11, weight: .regular, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .textSelection(.enabled)
                        .padding(12)
                }
                .frame(height: 170)
                .background(DeskBridgeTheme.codeBackground, in: RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(DeskBridgeTheme.hairline))
                .padding(.top, 12)
            } label: {
                HStack {
                    sectionHeader(title: "Diagnostics", subtitle: "Logs, probes, and counters when needed", systemImage: "doc.text.magnifyingglass")
                    Spacer()
                    Button {
                        copyDiagnostics()
                    } label: {
                        Label("Copy", systemImage: "doc.on.doc")
                    }
                    .buttonStyle(.bordered)
                }
            }
            .disclosureGroupStyle(.automatic)
        }
    }

    private func sectionSurface<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        content()
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(DeskBridgeTheme.hairline))
    }

    private func sectionHeader(title: String, subtitle: String, systemImage: String) -> some View {
        HStack(spacing: 10) {
            Image(systemName: systemImage)
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(DeskBridgeTheme.accent)
                .frame(width: 24, height: 24)
                .background(DeskBridgeTheme.iconBackground, in: RoundedRectangle(cornerRadius: 6))

            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(size: 15, weight: .semibold))
                Text(subtitle)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
        }
    }

    private func settingLabel(_ title: String, systemImage: String) -> some View {
        Label(title, systemImage: systemImage)
            .font(.system(size: 13, weight: .medium))
            .foregroundStyle(.secondary)
            .frame(width: 128, alignment: .leading)
    }

    private func styledTextField(_ placeholder: String, text: Binding<String>) -> some View {
        TextField(placeholder, text: text)
            .textFieldStyle(.plain)
            .font(.system(size: 13, weight: .medium))
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .background(DeskBridgeTheme.inputBackground, in: RoundedRectangle(cornerRadius: 7))
            .overlay(RoundedRectangle(cornerRadius: 7).stroke(DeskBridgeTheme.hairline))
            .textSelection(.enabled)
    }

    private func detailLabel(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 12, weight: .medium))
            .foregroundStyle(.secondary)
            .frame(width: 82, alignment: .leading)
    }

    private func detailValue(_ text: String, lineLimit: Int? = nil) -> some View {
        Text(text)
            .font(.system(size: 12, weight: .medium))
            .lineLimit(lineLimit)
            .truncationMode(.middle)
            .textSelection(.enabled)
    }

    private func copyDiagnostics() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(model.lastDiagnostics, forType: .string)
    }

    private struct LayoutMetrics {
        let scale: Double
        let localSize: CGSize
        let peerSize: CGSize
        let localOrigin: CGPoint
        let peerOrigin: CGPoint
    }

    private func layoutMetrics(for size: CGSize) -> LayoutMetrics {
        let padding = 20.0
        let boundsLeft = min(0, model.peerOffsetX)
        let boundsTop = min(0, model.peerOffsetY)
        let boundsRight = max(model.localDisplayWidth, model.peerOffsetX + model.peerDisplayWidth)
        let boundsBottom = max(model.localDisplayHeight, model.peerOffsetY + model.peerDisplayHeight)
        let boundsWidth = max(1, boundsRight - boundsLeft)
        let boundsHeight = max(1, boundsBottom - boundsTop)
        let widthScale = max(0.001, Double(size.width) - padding * 2) / boundsWidth
        let heightScale = max(0.001, Double(size.height) - padding * 2) / boundsHeight
        let scale = min(widthScale, heightScale, 0.10)
        let scaledWidth = boundsWidth * scale
        let scaledHeight = boundsHeight * scale
        let originX = max(padding, (Double(size.width) - scaledWidth) / 2)
        let originY = max(padding, (Double(size.height) - scaledHeight) / 2)
        let localOrigin = CGPoint(
            x: originX + (-boundsLeft) * scale,
            y: originY + (-boundsTop) * scale
        )
        let peerOrigin = CGPoint(
            x: localOrigin.x + model.peerOffsetX * scale,
            y: localOrigin.y + model.peerOffsetY * scale
        )

        return LayoutMetrics(
            scale: scale,
            localSize: CGSize(width: model.localDisplayWidth * scale, height: model.localDisplayHeight * scale),
            peerSize: CGSize(width: model.peerDisplayWidth * scale, height: model.peerDisplayHeight * scale),
            localOrigin: localOrigin,
            peerOrigin: peerOrigin
        )
    }

    private func screenDragGesture(scale: Double) -> some Gesture {
        DragGesture()
            .onChanged { value in
                if dragStartOffset == nil {
                    dragStartOffset = CGSize(width: model.peerOffsetX, height: model.peerOffsetY)
                    dragStartScale = scale
                }

                let start = dragStartOffset ?? .zero
                let deltaX = value.translation.width / dragStartScale
                let deltaY = value.translation.height / dragStartScale
                model.peerOffsetX = clamp(start.width + deltaX, lower: -model.peerDisplayWidth, upper: model.localDisplayWidth)
                model.peerOffsetY = clamp(start.height + deltaY, lower: -model.peerDisplayHeight, upper: model.localDisplayHeight)
            }
            .onEnded { _ in
                dragStartOffset = nil
                model.snapPeerToNearestEdge()
            }
    }

    private func clamp(_ value: Double, lower: Double, upper: Double) -> Double {
        min(max(value, lower), upper)
    }
}

enum DeskBridgeTheme {
    static let accent = Color(red: 0.80, green: 0.95, blue: 0.42)
    static let success = Color(red: 0.43, green: 0.84, blue: 0.47)
    static let warning = Color(red: 0.95, green: 0.65, blue: 0.35)
    static let localScreen = Color(red: 0.50, green: 0.66, blue: 0.90)
    static let peerScreen = Color(red: 0.86, green: 0.58, blue: 0.72)

    static let windowBackground = LinearGradient(
        colors: [
            Color(nsColor: .windowBackgroundColor),
            Color(nsColor: .underPageBackgroundColor).opacity(0.92),
        ],
        startPoint: .topLeading,
        endPoint: .bottomTrailing
    )
    static let pillBackground = Color(nsColor: .controlBackgroundColor).opacity(0.66)
    static let iconBackground = accent.opacity(0.12)
    static let inputBackground = Color(nsColor: .textBackgroundColor).opacity(0.52)
    static let canvasBackground = Color(nsColor: .textBackgroundColor).opacity(0.30)
    static let codeBackground = Color(nsColor: .textBackgroundColor).opacity(0.42)
    static let hairline = Color.primary.opacity(0.10)
}

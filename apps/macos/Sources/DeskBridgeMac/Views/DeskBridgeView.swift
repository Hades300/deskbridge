import SwiftUI

struct DeskBridgeView: View {
    @ObservedObject var model: DeskBridgeModel
    @State private var dragStartOffset: CGSize?
    @State private var dragStartScale: Double = 0.08

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            header
            modePicker
            connectionForm
            if model.mode == .server {
                layoutEditor
            }
            controlBar
            statusPanel
            diagnosticsPanel
        }
        .padding(24)
        .frame(minWidth: 720, idealWidth: 780, minHeight: 720)
    }

    private var header: some View {
        HStack(spacing: 12) {
            Image(systemName: model.connected ? "checkmark.circle.fill" : "bolt.horizontal.circle.fill")
                .font(.system(size: 26))
                .foregroundStyle(model.connected ? .green : .orange)

            VStack(alignment: .leading, spacing: 3) {
                Text("DeskBridge")
                    .font(.title2.weight(.semibold))
                Text(model.status)
                    .foregroundStyle(.secondary)
            }

            Spacer()
        }
    }

    private var modePicker: some View {
        Picker("Mode", selection: $model.mode) {
            ForEach(DeskBridgeMode.allCases) { mode in
                Text(mode.rawValue).tag(mode)
            }
        }
        .pickerStyle(.segmented)
        .onChange(of: model.mode) {
            model.save()
        }
    }

    private var connectionForm: some View {
        Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 12) {
            if model.mode == .client {
                GridRow {
                    Label("Server", systemImage: "desktopcomputer")
                        .foregroundStyle(.secondary)
                    TextField("192.168.2.5:24800", text: $model.server)
                        .textFieldStyle(.roundedBorder)
                }
            } else {
                GridRow {
                    Label("Listen", systemImage: "network")
                        .foregroundStyle(.secondary)
                    TextField("0.0.0.0:24800", text: $model.listenAddress)
                        .textFieldStyle(.roundedBorder)
                }
            }

            GridRow {
                Label("Local", systemImage: model.mode == .server ? "keyboard" : "macwindow")
                    .foregroundStyle(.secondary)
                TextField("mac", text: $model.screenName)
                    .textFieldStyle(.roundedBorder)
            }

            GridRow {
                Label("Peer", systemImage: "rectangle.connected.to.line.below")
                    .foregroundStyle(.secondary)
                TextField("windows", text: $model.peerScreenName)
                    .textFieldStyle(.roundedBorder)
            }

            GridRow {
                Label("Recovery", systemImage: "arrow.clockwise")
                    .foregroundStyle(.secondary)
                HStack(spacing: 18) {
                    Toggle("Auto reconnect", isOn: $model.autoReconnect)
                        .toggleStyle(.checkbox)
                    if model.mode == .server {
                        Toggle("Reverse remote wheel", isOn: $model.reverseScroll)
                            .toggleStyle(.checkbox)
                    }
                }
                .onChange(of: model.autoReconnect) { model.save() }
                .onChange(of: model.reverseScroll) { model.applyRuntimeInputSettings() }
            }

            GridRow {
                Label("Clipboard", systemImage: "doc.on.clipboard")
                    .foregroundStyle(.secondary)
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
                .onChange(of: model.clipboardEnabled) { model.save() }
                .onChange(of: model.clipboardText) { model.save() }
                .onChange(of: model.clipboardImage) { model.save() }
                .onChange(of: model.clipboardFiles) { model.save() }
            }

            if model.mode == .server {
                GridRow {
                    Label("Capture", systemImage: "cursorarrow.motionlines")
                        .foregroundStyle(.secondary)
                    HStack(spacing: 18) {
                        Toggle("Keyboard and mouse", isOn: $model.captureInput)
                            .toggleStyle(.checkbox)
                        Toggle("Route history", isOn: $model.debugLogging)
                            .toggleStyle(.checkbox)
                    }
                    .onChange(of: model.captureInput) { model.save() }
                    .onChange(of: model.debugLogging) { model.save() }
                }
            }
        }
    }

    private var layoutEditor: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Layout")
                    .font(.headline)
                Text(model.entryDescription)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                Spacer()
                Button {
                    model.snapPeerToNearestEdge()
                } label: {
                    Label("Snap", systemImage: "rectangle.2.swap")
                }
            }

            GeometryReader { proxy in
                let metrics = layoutMetrics(for: proxy.size)

                ZStack(alignment: .topLeading) {
                    RoundedRectangle(cornerRadius: 8)
                        .fill(.quaternary.opacity(0.28))

                    gridLines

                    screenBox(
                        title: model.screenName,
                        subtitle: model.localRoleLabel,
                        size: metrics.localSize,
                        glowEdge: model.localGlowEdge,
                        isLocal: true
                    )
                    .position(
                        x: metrics.localOrigin.x + metrics.localSize.width / 2,
                        y: metrics.localOrigin.y + metrics.localSize.height / 2
                    )

                    screenBox(
                        title: model.peerScreenName,
                        subtitle: model.peerRoleLabel,
                        size: metrics.peerSize,
                        glowEdge: model.peerGlowEdge,
                        isLocal: false
                    )
                    .position(
                        x: metrics.peerOrigin.x + metrics.peerSize.width / 2,
                        y: metrics.peerOrigin.y + metrics.peerSize.height / 2
                    )
                    .gesture(screenDragGesture(scale: metrics.scale))
                }
            }
            .frame(height: 190)
            .clipShape(RoundedRectangle(cornerRadius: 8))
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
            context.stroke(path, with: .color(.secondary.opacity(0.12)), lineWidth: 1)
        }
    }

    private func screenBox(
        title: String,
        subtitle: String,
        size: CGSize,
        glowEdge: LayoutPreviewEdge,
        isLocal: Bool
    ) -> some View {
        ZStack {
            RoundedRectangle(cornerRadius: 7)
                .fill(isLocal ? .blue.opacity(0.18) : .purple.opacity(0.16))
                .overlay(
                    RoundedRectangle(cornerRadius: 7)
                        .stroke(isLocal ? .blue.opacity(0.55) : .purple.opacity(0.50), lineWidth: 1)
                )

            glowStrip(edge: glowEdge, size: size)

            VStack(spacing: 3) {
                Text(title)
                    .font(.headline)
                    .lineLimit(1)
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .padding(8)
        }
        .frame(width: size.width, height: size.height)
        .shadow(color: .green.opacity(0.20), radius: 8)
    }

    @ViewBuilder
    private func glowStrip(edge: LayoutPreviewEdge, size: CGSize) -> some View {
        let color = Color.green.opacity(0.82)
        switch edge {
        case .left:
            HStack {
                Rectangle().fill(color).frame(width: 5)
                    .shadow(color: color, radius: 8)
                Spacer()
            }
        case .right:
            HStack {
                Spacer()
                Rectangle().fill(color).frame(width: 5)
                    .shadow(color: color, radius: 8)
            }
        case .top:
            VStack {
                Rectangle().fill(color).frame(height: 5)
                    .shadow(color: color, radius: 8)
                Spacer()
            }
        case .bottom:
            VStack {
                Spacer()
                Rectangle().fill(color).frame(height: 5)
                    .shadow(color: color, radius: 8)
            }
        }
    }

    private struct LayoutMetrics {
        let scale: Double
        let localSize: CGSize
        let peerSize: CGSize
        let localOrigin: CGPoint
        let peerOrigin: CGPoint
    }

    private func layoutMetrics(for size: CGSize) -> LayoutMetrics {
        let padding = 18.0
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

    private var controlBar: some View {
        HStack(spacing: 10) {
            Button {
                model.connect()
            } label: {
                Label(model.mode == .server ? "Start" : "Connect", systemImage: "play.fill")
            }
            .keyboardShortcut(.defaultAction)

            Button {
                model.disconnect()
            } label: {
                Label(model.mode == .server ? "Stop" : "Disconnect", systemImage: "stop.fill")
            }

            Button {
                model.runDiagnostics()
            } label: {
                Label("Diagnose", systemImage: "stethoscope")
            }

            Button {
                model.openAccessibilitySettings()
            } label: {
                Label("Accessibility", systemImage: "lock.shield")
            }

            Spacer()

            if model.mode == .server {
                Button {
                    model.writeDefaultConfig()
                } label: {
                    Label("Save Config", systemImage: "doc.badge.gearshape")
                }
            }
        }
        .buttonStyle(.bordered)
    }

    private var statusPanel: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Status")
                .font(.headline)

            Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 6) {
                GridRow {
                    Text(model.mode == .server ? "Listen" : "Target")
                        .foregroundStyle(.secondary)
                    Text(model.mode == .server ? model.listenAddress : model.normalizedServerAddress)
                        .textSelection(.enabled)
                }

                GridRow {
                    Text("Binary")
                        .foregroundStyle(.secondary)
                    Text(model.binaryPath)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                }

                GridRow {
                    Text("Last log")
                        .foregroundStyle(.secondary)
                    Text(model.lastLogLine.isEmpty ? "No client log yet." : model.lastLogLine)
                        .lineLimit(2)
                        .textSelection(.enabled)
                }
            }
            .font(.callout)
        }
    }

    private var diagnosticsPanel: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Diagnostics")
                .font(.headline)

            ScrollView {
                Text(model.lastDiagnostics)
                    .font(.system(.caption, design: .monospaced))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
                    .padding(10)
            }
            .frame(height: 150)
            .background(.quaternary.opacity(0.35))
            .clipShape(RoundedRectangle(cornerRadius: 8))
        }
    }
}

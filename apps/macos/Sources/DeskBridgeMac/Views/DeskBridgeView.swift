import SwiftUI

private enum LayoutDragTarget: Equatable {
    case local
    case peer
}

struct DeskBridgeView: View {
    @ObservedObject var model: DeskBridgeModel
    @State private var dragStartOffset: CGSize?
    @State private var dragTarget: LayoutDragTarget?

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
                let scale = layoutScale(for: proxy.size)
                let localSize = CGSize(width: model.localDisplayWidth * scale, height: model.localDisplayHeight * scale)
                let peerSize = CGSize(width: model.peerDisplayWidth * scale, height: model.peerDisplayHeight * scale)
                let localOrigin = CGPoint(
                    x: proxy.size.width / 2 - localSize.width / 2,
                    y: proxy.size.height / 2 - localSize.height / 2
                )
                let peerOrigin = CGPoint(
                    x: localOrigin.x + model.peerOffsetX * scale,
                    y: localOrigin.y + model.peerOffsetY * scale
                )

                ZStack(alignment: .topLeading) {
                    RoundedRectangle(cornerRadius: 8)
                        .fill(.quaternary.opacity(0.28))

                    gridLines

                    screenBox(
                        title: model.screenName,
                        subtitle: model.localRoleLabel,
                        size: localSize,
                        glowEdge: model.localGlowEdge,
                        isLocal: true
                    )
                    .position(x: localOrigin.x + localSize.width / 2, y: localOrigin.y + localSize.height / 2)
                    .gesture(screenDragGesture(target: .local, scale: scale))

                    screenBox(
                        title: model.peerScreenName,
                        subtitle: model.peerRoleLabel,
                        size: peerSize,
                        glowEdge: model.peerGlowEdge,
                        isLocal: false
                    )
                    .position(x: peerOrigin.x + peerSize.width / 2, y: peerOrigin.y + peerSize.height / 2)
                    .gesture(screenDragGesture(target: .peer, scale: scale))
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

    private func layoutScale(for size: CGSize) -> Double {
        let totalWidth = model.localDisplayWidth + model.peerDisplayWidth
        let totalHeight = model.localDisplayHeight + model.peerDisplayHeight
        let widthScale = max(0.04, Double(size.width - 80) / totalWidth)
        let heightScale = max(0.04, Double(size.height - 50) / totalHeight)
        return min(widthScale, heightScale, 0.10)
    }

    private func screenDragGesture(target: LayoutDragTarget, scale: Double) -> some Gesture {
        DragGesture()
            .onChanged { value in
                if dragStartOffset == nil || dragTarget != target {
                    dragStartOffset = CGSize(width: model.peerOffsetX, height: model.peerOffsetY)
                    dragTarget = target
                }

                let start = dragStartOffset ?? .zero
                let deltaX = value.translation.width / scale
                let deltaY = value.translation.height / scale
                switch target {
                case .peer:
                    model.peerOffsetX = start.width + deltaX
                    model.peerOffsetY = start.height + deltaY
                case .local:
                    model.peerOffsetX = start.width - deltaX
                    model.peerOffsetY = start.height - deltaY
                }
            }
            .onEnded { _ in
                dragStartOffset = nil
                dragTarget = nil
                model.snapPeerToNearestEdge()
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

import SwiftUI

struct DeskBridgeView: View {
    @ObservedObject var model: DeskBridgeModel

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            header
            connectionForm
            controlBar
            statusPanel
            diagnosticsPanel
        }
        .padding(24)
        .frame(minWidth: 560, idealWidth: 600, minHeight: 500)
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

    private var connectionForm: some View {
        Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 12) {
            GridRow {
                Label("Server", systemImage: "desktopcomputer")
                    .foregroundStyle(.secondary)
                TextField("192.168.2.5:24800", text: $model.server)
                    .textFieldStyle(.roundedBorder)
            }

            GridRow {
                Label("Screen", systemImage: "macwindow")
                    .foregroundStyle(.secondary)
                TextField("mac", text: $model.screenName)
                    .textFieldStyle(.roundedBorder)
            }

            GridRow {
                Label("Recovery", systemImage: "arrow.clockwise")
                    .foregroundStyle(.secondary)
                Toggle("Auto reconnect", isOn: $model.autoReconnect)
                    .toggleStyle(.checkbox)
                    .onChange(of: model.autoReconnect) {
                        model.save()
                    }
            }
        }
    }

    private var controlBar: some View {
        HStack(spacing: 10) {
            Button {
                model.connect()
            } label: {
                Label("Connect", systemImage: "play.fill")
            }
            .keyboardShortcut(.defaultAction)

            Button {
                model.disconnect()
            } label: {
                Label("Disconnect", systemImage: "stop.fill")
            }

            Button {
                model.runDiagnostics()
            } label: {
                Label("Diagnose", systemImage: "stethoscope")
            }

            Spacer()

            Button {
                model.writeDefaultConfig()
            } label: {
                Label("Save Config", systemImage: "doc.badge.gearshape")
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
                    Text("Target")
                        .foregroundStyle(.secondary)
                    Text(model.normalizedServerAddress)
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

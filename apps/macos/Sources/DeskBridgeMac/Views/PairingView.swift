import AppKit
import SwiftUI

/// Sheet that walks the user through discovering a server and pairing with it
/// by confirming a short code. It drives the `deskbridge pair` CLI through the
/// model, so the flow stays in lock-step with the verified pairing engine.
struct PairingView: View {
    @ObservedObject var model: DeskBridgeModel
    @Binding var isPresented: Bool
    @State private var manualAddress: String = ""

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(DeskBridgeTheme.hairline)
            ScrollView {
                content
                    .padding(20)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .frame(width: 480, height: 560)
        .background(DeskBridgeTheme.windowBackground.ignoresSafeArea())
        .onAppear {
            if model.discoveredServers.isEmpty {
                model.discoverServers()
            }
        }
    }

    private var header: some View {
        HStack(spacing: 12) {
            Image(systemName: "qrcode")
                .font(.system(size: 18, weight: .semibold))
                .foregroundStyle(DeskBridgeTheme.accent)
                .frame(width: 32, height: 32)
                .background(DeskBridgeTheme.iconBackground, in: RoundedRectangle(cornerRadius: 8))

            VStack(alignment: .leading, spacing: 2) {
                Text("Pair a device")
                    .font(.system(size: 17, weight: .semibold))
                Text("Confirm a short code to connect securely")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(.secondary)
            }

            Spacer()

            Button {
                model.cancelPairing()
                isPresented = false
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 12, weight: .bold))
                    .foregroundStyle(.secondary)
                    .frame(width: 26, height: 26)
                    .background(DeskBridgeTheme.pillBackground, in: Circle())
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 16)
    }

    @ViewBuilder
    private var content: some View {
        switch model.pairingPhase {
        case .idle, .connecting:
            discoverContent
        case .awaitingConfirmation(let code):
            confirmContent(code: code)
        case .succeeded(let server):
            successContent(server: server)
        case .failed(let message):
            failureContent(message: message)
        }
    }

    // MARK: - Discover

    private var discoverContent: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack {
                Text("On your network")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(.secondary)
                Spacer()
                Button {
                    model.discoverServers()
                } label: {
                    HStack(spacing: 6) {
                        if model.isDiscovering {
                            ProgressView().controlSize(.small)
                        } else {
                            Image(systemName: "arrow.clockwise")
                        }
                        Text(model.isDiscovering ? "Scanning…" : "Scan")
                    }
                }
                .buttonStyle(.bordered)
                .disabled(model.isDiscovering)
            }

            if model.discoveredServers.isEmpty {
                emptyDiscovery
            } else {
                VStack(spacing: 10) {
                    ForEach(model.discoveredServers) { server in
                        serverCard(server)
                    }
                }
            }

            Divider().overlay(DeskBridgeTheme.hairline)

            VStack(alignment: .leading, spacing: 8) {
                Text("Or enter an address")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(.secondary)

                HStack(spacing: 10) {
                    TextField("192.168.2.5:24800", text: $manualAddress)
                        .textFieldStyle(.plain)
                        .font(.system(size: 13, weight: .medium))
                        .padding(.horizontal, 10)
                        .padding(.vertical, 8)
                        .background(DeskBridgeTheme.inputBackground, in: RoundedRectangle(cornerRadius: 7))
                        .overlay(RoundedRectangle(cornerRadius: 7).stroke(DeskBridgeTheme.hairline))
                        .onSubmit { startManual() }

                    Button {
                        startManual()
                    } label: {
                        Text("Pair")
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(DeskBridgeTheme.accent)
                    .disabled(manualAddress.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }

            if case .connecting = model.pairingPhase {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Connecting…")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    private func serverCard(_ server: DiscoveredServer) -> some View {
        Button {
            model.startPairing(with: server.address)
        } label: {
            HStack(spacing: 12) {
                Image(systemName: "desktopcomputer")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(DeskBridgeTheme.accent)
                    .frame(width: 34, height: 34)
                    .background(DeskBridgeTheme.iconBackground, in: RoundedRectangle(cornerRadius: 8))

                VStack(alignment: .leading, spacing: 3) {
                    Text(server.name)
                        .font(.system(size: 14, weight: .semibold))
                    Text(server.address)
                        .font(.system(size: 12, weight: .medium, design: .monospaced))
                        .foregroundStyle(.secondary)
                }

                Spacer()

                if let version = server.version {
                    Text(version)
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(DeskBridgeTheme.pillBackground, in: Capsule())
                }

                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(.tertiary)
            }
            .padding(12)
            .background(DeskBridgeTheme.inputBackground, in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(DeskBridgeTheme.hairline))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private var emptyDiscovery: some View {
        VStack(spacing: 8) {
            Image(systemName: "dot.radiowaves.left.and.right")
                .font(.system(size: 22))
                .foregroundStyle(.secondary)
            Text(model.isDiscovering ? "Looking for servers…" : "No servers found yet")
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(.secondary)
            Text("Make sure the other device is running DeskBridge on the same network.")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.tertiary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 24)
        .background(DeskBridgeTheme.inputBackground.opacity(0.4), in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(DeskBridgeTheme.hairline))
    }

    // MARK: - Confirm

    private func confirmContent(code: String) -> some View {
        VStack(spacing: 22) {
            Spacer(minLength: 8)

            VStack(spacing: 8) {
                Image(systemName: "lock.shield")
                    .font(.system(size: 26, weight: .semibold))
                    .foregroundStyle(DeskBridgeTheme.accent)
                Text("Same code on both devices?")
                    .font(.system(size: 16, weight: .semibold))
                Text("Check that this matches the code on the other device, then confirm.")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Text(code)
                .font(.system(size: 46, weight: .bold, design: .monospaced))
                .tracking(6)
                .padding(.vertical, 18)
                .padding(.horizontal, 26)
                .frame(maxWidth: .infinity)
                .background(DeskBridgeTheme.codeBackground, in: RoundedRectangle(cornerRadius: 14))
                .overlay(RoundedRectangle(cornerRadius: 14).stroke(DeskBridgeTheme.accent.opacity(0.4), lineWidth: 1))

            HStack(spacing: 12) {
                Button {
                    model.cancelPairing()
                } label: {
                    Text("Not the same").frame(maxWidth: .infinity)
                }
                .buttonStyle(.bordered)

                Button {
                    model.confirmPairing()
                } label: {
                    Text("They match").frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .tint(DeskBridgeTheme.accent)
                .keyboardShortcut(.defaultAction)
            }

            Spacer(minLength: 8)
        }
        .frame(maxWidth: .infinity)
    }

    // MARK: - Result

    private func successContent(server: String) -> some View {
        VStack(spacing: 18) {
            Spacer()
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 52))
                .foregroundStyle(DeskBridgeTheme.success)
            VStack(spacing: 6) {
                Text("Paired")
                    .font(.system(size: 18, weight: .semibold))
                Text("Connected to \(server). The session is now encrypted.")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
            }

            VStack(spacing: 10) {
                Button {
                    isPresented = false
                    model.pairingPhase = .idle
                    model.connect()
                } label: {
                    Text("Connect now").frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .tint(DeskBridgeTheme.accent)

                Button {
                    isPresented = false
                    model.pairingPhase = .idle
                } label: {
                    Text("Done").frame(maxWidth: .infinity)
                }
                .buttonStyle(.bordered)
            }
            .frame(maxWidth: 260)

            Spacer()
        }
        .frame(maxWidth: .infinity)
    }

    private func failureContent(message: String) -> some View {
        VStack(spacing: 16) {
            Spacer()
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 44))
                .foregroundStyle(DeskBridgeTheme.warning)
            Text("Pairing didn't finish")
                .font(.system(size: 16, weight: .semibold))
            Text(message)
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)

            Button {
                model.pairingPhase = .idle
            } label: {
                Text("Try again").frame(maxWidth: 220)
            }
            .buttonStyle(.borderedProminent)
            .tint(DeskBridgeTheme.accent)

            Spacer()
        }
        .frame(maxWidth: .infinity)
    }

    private func startManual() {
        let trimmed = manualAddress.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return }
        model.startPairing(with: trimmed)
    }
}

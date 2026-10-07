import AppKit
import SwiftUI

final class SettingsModel: ObservableObject {
    @Published private(set) var status: SyncStatus?
    @Published private(set) var busy = false
    @Published var message: String?
    @Published private(set) var pastingAllowed = Paster.isTrusted

    private let client = SyncClient()
    private let store: HistoryStore

    init(store: HistoryStore) {
        self.store = store
    }

    func load() {
        pastingAllowed = Paster.isTrusted
        busy = true
        client.status { status in
            self.status = status
            self.busy = false
        }
    }

    func remove(_ device: SyncDevice) {
        run(["revoke", device.id], success: device.isThisDevice ? "Logged out." : "\(device.name) removed.")
    }

    func clear(everywhere: Bool) {
        run(everywhere ? ["clear", "--everywhere"] : ["clear"],
            success: everywhere ? "History cleared on all devices." : "History cleared on this Mac.")
    }

    private func run(_ arguments: [String], success: String) {
        busy = true
        client.perform(arguments) { error in
            self.message = error.map { "Failed: \($0)" } ?? success
            self.load()
        }
    }
}

final class SettingsWindowController {
    private let store: HistoryStore
    private var window: NSWindow?

    init(store: HistoryStore) {
        self.store = store
    }

    func show() {
        if window == nil {
            let window = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 560, height: 640),
                styleMask: [.titled, .closable, .miniaturizable, .resizable],
                backing: .buffered,
                defer: false
            )
            window.title = "Pastazzo Settings"
            window.isReleasedWhenClosed = false
            window.contentView = NSHostingView(rootView: SettingsView(model: SettingsModel(store: store)))
            window.center()
            self.window = window
        }
        NSApp.activate(ignoringOtherApps: true)
        window?.makeKeyAndOrderFront(nil)
    }
}

/// What a destructive button asks before acting.
private struct Confirmation {
    let title: String
    let message: String
    let action: String
    let perform: () -> Void
}

struct SettingsView: View {
    @ObservedObject var model: SettingsModel
    @State private var confirmation: Confirmation?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                if let message = model.message {
                    Text(message).foregroundColor(.secondary)
                }
                syncSections
                section("Pasting") {
                    Text(model.pastingAllowed
                        ? "Double-click an item, or select it and press Return, to paste it into the app you were using."
                        : "To paste with a double-click or Return, allow Pastazzo in System Preferences → Security & Privacy → Privacy → Accessibility.")
                        .fixedSize(horizontal: false, vertical: true)
                    if !model.pastingAllowed {
                        Button("Open Accessibility Settings") {
                            Paster.requestAccess()
                            if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") {
                                NSWorkspace.shared.open(url)
                            }
                        }
                    }
                }
                section("Shortcut") {
                    Text("⇧⌥V opens Pastazzo from anywhere. Click the menu bar icon to open it, right-click it for the menu.")
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(minWidth: 480, minHeight: 520)
        .onAppear { model.load() }
        .alert(
            confirmation?.title ?? "",
            isPresented: Binding(get: { confirmation != nil }, set: { if !$0 { confirmation = nil } }),
            presenting: confirmation,
            actions: { confirmation in
                Button(confirmation.action, role: .destructive) { confirmation.perform() }
                Button("Cancel", role: .cancel) {}
            },
            message: { confirmation in Text(confirmation.message) }
        )
    }

    @ViewBuilder
    private var syncSections: some View {
        if let status = model.status, status.loggedIn {
            section("Account") {
                row("Username", status.username ?? "")
                row("Server", status.serverUrl ?? "")
                row("This Mac", status.thisDevice?.name ?? "")
            }
            section("Devices") {
                Text("Devices syncing with this account. Each shows its key fingerprint: it must match what that device shows for itself.")
                    .font(.caption)
                    .foregroundColor(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if let error = status.devicesError {
                    Text("Couldn't reach the server: \(error)").foregroundColor(.red)
                }
                ForEach(status.devices ?? []) { device in
                    HStack(alignment: .center) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(device.isThisDevice ? "\(device.name) (this Mac)" : device.name)
                            fingerprint(device.fingerprint).font(.system(.caption, design: .monospaced))
                        }
                        Spacer()
                        Button(device.isThisDevice ? "Log Out" : "Remove", role: .destructive) {
                            confirmation = Confirmation(
                                title: device.isThisDevice ? "Log out of sync?" : "Remove \(device.name)?",
                                message: device.isThisDevice
                                    ? "This Mac stops syncing and forgets the account. You can log in again later."
                                    : "\(device.name) stops syncing with this account: it can't send or receive anything any more.",
                                action: device.isThisDevice ? "Log Out" : "Remove",
                                perform: { model.remove(device) }
                            )
                        }
                        .disabled(model.busy)
                    }
                    .padding(.vertical, 2)
                }
            }
            section("End-to-End Encryption") {
                Text("Your clipboard is encrypted on your devices with the account key, which never leaves them. Its fingerprint must be the same on every device: the server only ever stores ciphertext it can't read.")
                    .font(.caption)
                    .foregroundColor(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                labeledFingerprint("Account key", status.accountKeyFingerprint)
                labeledFingerprint("Server", status.serverFingerprint)
                labeledFingerprint("This Mac", status.thisDevice?.fingerprint)
            }
            section("History") {
                HStack {
                    Button("Clear on This Mac") { model.clear(everywhere: false) }
                    Button("Clear on All Devices…", role: .destructive) {
                        confirmation = Confirmation(
                            title: "Clear the history on all devices?",
                            message: "Every device empties its clipboard history as it syncs, and the server deletes what it stores.",
                            action: "Clear Everywhere",
                            perform: { model.clear(everywhere: true) }
                        )
                    }
                }
                .disabled(model.busy)
            }
        } else if model.busy || model.status == nil {
            ProgressView().frame(maxWidth: .infinity)
        } else {
            section("Sync") {
                Text("This Mac isn't syncing. Set it up with `pastazzo-sync join` (new account) or `pastazzo-sync login` (another device of an account).")
                    .fixedSize(horizontal: false, vertical: true)
                if let error = model.status?.error {
                    Text(error).font(.caption).foregroundColor(.secondary)
                }
            }
        }
    }

    private func section<Content: View>(_ title: String, @ViewBuilder content: () -> Content) -> some View {
        GroupBox(label: Text(title).font(.headline)) {
            VStack(alignment: .leading, spacing: 8, content: content)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(6)
        }
    }

    private func row(_ title: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title).foregroundColor(.secondary).frame(width: 100, alignment: .leading)
            Text(value).textSelection(.enabled)
        }
    }

    private func fingerprint(_ value: String) -> some View {
        Text(value).foregroundColor(.secondary).textSelection(.enabled)
    }

    private func labeledFingerprint(_ title: String, _ value: String?) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title).foregroundColor(.secondary).frame(width: 100, alignment: .leading)
            Text(value ?? "").font(.system(.body, design: .monospaced)).textSelection(.enabled)
        }
    }
}

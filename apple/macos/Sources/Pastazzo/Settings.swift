import AppKit
import SwiftUI
import CoreImage.CIFilterBuiltins

final class SettingsModel: ObservableObject {
    @Published private(set) var status: SyncStatus?
    @Published private(set) var busy = false
    @Published var message: String?
    @Published private(set) var pastingAllowed = Paster.isTrusted
    @Published var server = UserDefaults.standard.string(forKey: "syncServer") ?? ""
    @Published var username = UserDefaults.standard.string(forKey: "syncUsername") ?? ""
    @Published var fingerprint = UserDefaults.standard.string(forKey: "syncFingerprint") ?? ""
    @Published var password = ""
    @Published private(set) var approvalCode: String?
    @Published private(set) var connecting = false
    @Published var pairingPresented = false
    @Published private(set) var pairingImage: NSImage?
    @Published private(set) var pairingName: String?
    @Published private(set) var pairingCode: String?
    @Published private(set) var pairingExpiry: Date?
    @Published private(set) var pairingError: String?
    @Published private(set) var pairingCompleted = false
    @Published private(set) var pairingBusy = false
    private var pairingTimer: Timer?
    private var pollingPair = false

    private let client = SyncClient()
    private let store: HistoryStore

    init(store: HistoryStore) {
        self.store = store
    }

    func load() {
        guard !connecting else { return }
        pastingAllowed = Paster.isTrusted
        busy = true
        client.status { status in
            self.status = status
            self.busy = false
        }
    }

    func connect() {
        connecting = true
        message = nil
        approvalCode = nil
        let secret = password
        password = ""
        let server = server.trimmingCharacters(in: .whitespacesAndNewlines)
        let username = username.trimmingCharacters(in: .whitespacesAndNewlines)
        let fingerprint = fingerprint.trimmingCharacters(in: .whitespacesAndNewlines)
        client.login(server: server, fingerprint: fingerprint, username: username, password: secret,
                     onCode: { self.approvalCode = $0 }) { error in
            self.connecting = false
            self.approvalCode = nil
            self.message = error.map { "Couldn't connect: \($0)" } ?? "This Mac is connected."
            if error == nil {
                UserDefaults.standard.set(server, forKey: "syncServer")
                UserDefaults.standard.set(username, forKey: "syncUsername")
                UserDefaults.standard.set(fingerprint, forKey: "syncFingerprint")
            }
            self.load()
        }
    }

    func startPairing() {
        guard !pairingBusy else { return }
        pairingPresented = true
        pairingTimer?.invalidate()
        pairingImage = nil; pairingName = nil; pairingCode = nil; pairingError = nil
        pairingCompleted = false; pairingBusy = true
        client.createPairing { [self] link, expiry, error in
            self.pairingBusy = false
            guard self.pairingPresented else {
                self.client.perform(["pair", "cancel"]) { _ in }; return
            }
            self.pairingError = error
            guard let link, let expiry else { return }
            let filter = CIFilter.qrCodeGenerator()
            filter.message = Data(link.utf8)
            filter.correctionLevel = "M"
            if let output = filter.outputImage,
               let cg = CIContext().createCGImage(output.transformed(by: CGAffineTransform(scaleX: 8, y: 8)),
                   from: output.extent.applying(CGAffineTransform(scaleX: 8, y: 8))) {
                self.pairingImage = NSImage(cgImage: cg, size: NSSize(width: cg.width, height: cg.height))
            } else { self.pairingError = "Couldn't draw the QR. Generate a new one."; return }
            self.pairingExpiry = Date(timeIntervalSince1970: expiry / 1000)
            self.pairingTimer = Timer.scheduledTimer(withTimeInterval: 1.5, repeats: true) { [weak self] _ in self?.pollPairing() }
        }
    }

    private func pollPairing() {
        guard !pollingPair, !pairingBusy else { return }
        pollingPair = true
        client.pairingStatus { status, error in
            self.pollingPair = false
            if let error { self.pairingError = error; self.pairingImage = nil; self.pairingTimer?.invalidate(); return }
            guard let status else { return }
            self.pairingName = status.name
            self.pairingCode = status.code
            if status.completed { self.pairingCompleted = true; self.pairingImage = nil; self.pairingTimer?.invalidate(); self.load() }
        }
    }

    func approvePairing() {
        guard let code = pairingCode, !pairingBusy else { return }
        pairingBusy = true
        client.perform(["pair", "approve", "--code", code]) { error in
            self.pairingBusy = false
            self.pairingError = error
            if error == nil { self.pairingCompleted = true; self.pairingImage = nil; self.pairingTimer?.invalidate(); self.load() }
        }
    }

    func stopPairing() {
        pairingTimer?.invalidate()
        pairingImage = nil
        if !pairingCompleted && !pairingBusy { client.perform(["pair", "cancel"]) { _ in } }
    }

    func approve(_ device: SyncPending) {
        run(["approve", device.id, "--yes"], success: "Device approved.")
    }

    func reject(_ device: SyncPending) {
        run(["revoke", device.id], success: "Device rejected.")
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
    private var model: SettingsModel?

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
            let model = SettingsModel(store: store)
            self.model = model
            window.contentView = NSHostingView(rootView: SettingsView(model: model))
            window.center()
            self.window = window
        }
        NSApp.activate(ignoringOtherApps: true)
        window?.makeKeyAndOrderFront(nil)
    }

    func showPairing() {
        show()
        model?.startPairing()
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
        .sheet(isPresented: $model.pairingPresented, onDismiss: { model.stopPairing() }) {
            PairingView(model: model)
        }
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
            section("Connect an iPhone") {
                Text("Show a QR, scan it in Pastazzo on your iPhone, then confirm the connection here.")
                    .font(.caption).foregroundColor(.secondary)
                Button("Show Pairing QR") { model.startPairing() }
                    .disabled(model.busy || !(status.canApprove ?? false))
            }
            if let waiting = status.pendingDevices, !waiting.isEmpty {
                section("Waiting for Approval") {
                    Text(status.canApprove ?? false
                        ? "A device logged in with your password and asks to join. Approve it only if it shows exactly the same code: the password alone doesn't let it in."
                        : "This Mac's account was created before approvals existed, so it can't approve: create the account again to use them.")
                        .font(.caption)
                        .foregroundColor(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    ForEach(waiting) { device in
                        HStack {
                            VStack(alignment: .leading, spacing: 2) {
                                Text("New device")
                                Text(device.code).font(.system(.title3, design: .monospaced)).textSelection(.enabled)
                            }
                            Spacer()
                            Button("Reject") { model.reject(device) }.disabled(model.busy)
                            if status.canApprove ?? false {
                                Button("Approve…") {
                                    confirmation = Confirmation(
                                        title: "Approve this device?",
                                        message: "Approve it only if the new device shows exactly this code:\n\n\(device.code)",
                                        action: "Approve",
                                        perform: { model.approve(device) }
                                    )
                                }
                                .disabled(model.busy)
                            }
                        }
                    }
                }
            }
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
        } else if model.connecting {
            section("Connect This Mac") {
                if let code = model.approvalCode {
                    Text("On your Mac Pro or another connected device, open Pastazzo Settings and approve only the device with this code.")
                        .fixedSize(horizontal: false, vertical: true)
                    Text(code).font(.system(.title, design: .monospaced)).textSelection(.enabled)
                    ProgressView("Waiting for approval…")
                } else {
                    ProgressView("Connecting securely…")
                }
            }
        } else if model.busy || model.status == nil {
            ProgressView().frame(maxWidth: .infinity)
        } else {
            section("Sync") {
                Text("Connect this Mac to your Pastazzo account. Your other device must approve it before any clipboard content can sync.")
                    .fixedSize(horizontal: false, vertical: true)
                TextField("Server URL", text: $model.server)
                TextField("Username", text: $model.username)
                SecureField("Account password", text: $model.password)
                TextField("Pinned server fingerprint", text: $model.fingerprint)
                    .font(.system(.body, design: .monospaced))
                Text("Copy the server fingerprint from a device you trust. It identifies your server independently of the network.")
                    .font(.caption).foregroundColor(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Button("Connect This Mac") { model.connect() }
                    .disabled(model.server.isEmpty || model.username.isEmpty || model.password.isEmpty || model.fingerprint.isEmpty)
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

import SwiftUI

struct SettingsView: View {
    @ObservedObject var model: HistoryModel
    var scanOnAppear = false
    @Environment(\.dismiss) private var dismiss
    @State private var server = ""
    @State private var username = ""
    @State private var fingerprint = ""
    @State private var password = ""
    @State private var clear = false
    @State private var logout = false
    @State private var scanner = false

    var body: some View {
        NavigationStack {
            Form {
                if let code = model.approvalCode {
                    Section("Waiting for approval") {
                        Text("Open Pastazzo on an already connected device. Approve this iPhone only if both devices show the same code.")
                        Text(code).font(.title2.monospaced()).textSelection(.enabled)
                        ProgressView("Waiting…")
                    }
                } else if model.connected {
                    Section("Connected") {
                        Label(model.device, systemImage: "iphone")
                        Label("End-to-end encrypted", systemImage: "lock.shield")
                    }
                    Section {
                        Text(model.accountFingerprint).font(.footnote.monospaced()).textSelection(.enabled)
                    } header: { Text("Account fingerprint") } footer: {
                        Text("This must match the account fingerprint shown on your other devices.")
                    }
                    Section {
                        Button("Disconnect this iPhone", role: .destructive) { logout = true }.disabled(model.busy)
                    }
                } else {
                    Section {
                        Button { scanner = true } label: {
                            Label("Scan Mac QR", systemImage: "qrcode.viewfinder").font(.headline)
                        }.disabled(model.busy)
                        Text("On your Mac, open Pastazzo Settings → Show Pairing QR. Scan it here, then confirm on the Mac.")
                            .foregroundStyle(.secondary)
                        if model.busy { ProgressView("Connecting securely…") }
                    } header: { Text("Connect with your Mac") } footer: {
                        Text("No password needed. Your copies stay end-to-end encrypted.")
                    }
                    Section {
                        DisclosureGroup("Connect manually") {
                        TextField("Server URL", text: $server).textContentType(.URL).keyboardType(.URL)
                        TextField("Username", text: $username).textContentType(.username)
                        SecureField("Account password", text: $password).textContentType(.password)
                        TextField("Pinned server fingerprint", text: $fingerprint).font(.footnote.monospaced())
                        Button("Connect this iPhone") {
                            let secret = password
                            password = ""
                            Task {
                                await model.login(server: server.trimmingCharacters(in: .whitespacesAndNewlines),
                                    fingerprint: fingerprint.trimmingCharacters(in: .whitespacesAndNewlines),
                                    username: username.trimmingCharacters(in: .whitespacesAndNewlines), password: secret)
                                await model.load()
                            }
                        }
                        .disabled(model.busy || server.isEmpty || username.isEmpty || fingerprint.isEmpty || password.isEmpty)
                        if model.busy { ProgressView("Connecting securely…") }
                        }
                    } footer: {
                        Text("Use the server and fingerprint from a device you trust. After login, that device must approve this iPhone.")
                    }
                    .textInputAutocapitalization(.never).autocorrectionDisabled()
                }
                if let message = model.message { Section { Text(message).foregroundStyle(.secondary) } }
                Section {
                    Text("Pastazzo receives copies while the app is open. It only reads your clipboard when you paste, and only writes to it when you tap Copy.")
                } header: { Text("Clipboard on iOS") }
                Section {
                    Button("Clear history on this iPhone", role: .destructive) { clear = true }.disabled(model.busy)
                }
            }
            .navigationTitle("Settings").navigationBarTitleDisplayMode(.inline)
            .onAppear { if scanOnAppear && !model.connected && !model.busy { scanner = true } }
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
            .sheet(isPresented: $scanner) {
                PairingScanner { link in
                    scanner = false
                    Task { await model.pair(link: link) }
                }
            }
            .confirmationDialog("Clear this iPhone's history?", isPresented: $clear, titleVisibility: .visible) {
                Button("Clear history", role: .destructive) {
                    Task { do { _ = try await model.client.call("clear_local"); await model.load(sync: false) }
                        catch { model.message = error.localizedDescription } }
                }
            }
            .confirmationDialog("Disconnect this iPhone?", isPresented: $logout, titleVisibility: .visible) {
                Button("Disconnect", role: .destructive) {
                    Task { do { _ = try await model.client.call("logout"); await model.load(sync: false) }
                        catch { model.message = error.localizedDescription } }
                }
            }
        }
    }
}

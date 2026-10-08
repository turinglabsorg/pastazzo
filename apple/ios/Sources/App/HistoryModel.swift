import SwiftUI
import UIKit

struct MobileItem: Decodable, Identifiable {
    let id: String
    let createdAt: Double
    let origin: String
    let kind: String
    let preview: String
    let size: Int
    let queued: Bool
    var date: Date { Date(timeIntervalSince1970: createdAt / 1000) }
}

@MainActor
final class HistoryModel: ObservableObject {
    @Published var items: [MobileItem] = []
    @Published var connected = false
    @Published var busy = false
    @Published var message: String?
    @Published var approvalCode: String?
    @Published var accountFingerprint = ""
    @Published var device = UIDevice.current.name
    let client: MobileClient

    init(client: MobileClient) { self.client = client }

    func load(sync: Bool = true) async {
        guard !busy else { return }
        busy = true
        defer { busy = false }
        do {
            let status = try await client.call("status")
            connected = status["logged_in"] as? Bool ?? false
            device = status["device_name"] as? String ?? UIDevice.current.name
            accountFingerprint = status["account_fingerprint"] as? String ?? ""
            if sync && connected {
                do { _ = try await client.call("refresh"); message = nil }
                catch { message = "Couldn't sync: \(error.localizedDescription)" }
            }
            try await readHistory()
        } catch { message = error.localizedDescription }
    }

    private func readHistory() async throws {
        let result = try await client.call("history")
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        items = try decoder.decode([MobileItem].self, from: JSONSerialization.data(withJSONObject: result["items"] ?? []))
    }

    func save(text: String) async { await save(fields: ["text": text]) }

    func pasteFromClipboard() async {
        while UIApplication.shared.applicationState != .active {
            do { try await Task.sleep(nanoseconds: 50_000_000) }
            catch { return }
        }
        let clipboard = UIPasteboard.general
        let hasText = clipboard.hasStrings
        let text = hasText ? clipboard.string : nil
        let image = !hasText && clipboard.hasImages ? clipboard.image : nil
        guard text != nil || image != nil else {
            message = "Copy text or an image first. If iOS asks, allow Pastazzo to paste."
            return
        }
        while busy {
            do { try await Task.sleep(nanoseconds: 50_000_000) }
            catch { return }
        }
        guard !Task.isCancelled else { return }
        if let text { await save(text: text) }
        else if let image { await save(image: image) }
    }
    func save(image: UIImage) async {
        guard let data = image.pngData() else { message = "This image couldn't be read."; return }
        await save(fields: ["kind": "image", "mime": "image/png", "data": Self.base64URL(data)])
    }

    private func save(fields: [String: Any]) async {
        guard !busy else { return }
        busy = true
        defer { busy = false }
        do {
            var fields = fields
            fields["name"] = device
            let result = try await client.call("save", fields)
            message = result["queued"] as? Bool == true ? "Saved. Sync will retry when you reconnect." : (connected ? "Saved and synced." : "Saved on this iPhone.")
            try await readHistory()
        } catch { message = error.localizedDescription }
    }

    func copy(_ item: MobileItem) async {
        do {
            let result = try await client.call("item", ["id": item.id])
            guard let content = result["item"] as? [String: Any] else { return }
            if item.kind == "text", let text = content["text"] as? String {
                UIPasteboard.general.setItems([["public.utf8-plain-text": text]], options: [.localOnly: true])
            } else if let encoded = content["data"] as? String, let data = Self.decodeBase64URL(encoded), let image = UIImage(data: data) {
                UIPasteboard.general.setItems([["public.png": image.pngData() ?? data]], options: [.localOnly: true])
            }
            message = "Copied."
        } catch { message = error.localizedDescription }
    }

    func login(server: String, fingerprint: String, username: String, password: String) async {
        await connect(operation: "login", fields: ["server": server, "fingerprint": fingerprint,
            "username": username, "password": password, "name": UIDevice.current.name])
    }

    func pair(link: String) async {
        await connect(operation: "pair", fields: ["link": link, "name": UIDevice.current.name])
        await load()
    }

    private func connect(operation: String, fields: [String: Any]) async {
        guard !busy else { return }
        busy = true
        message = nil
        let monitor = Task { [weak self] in
            while !Task.isCancelled {
                if let self, let code = try? String(contentsOf: self.client.root.appendingPathComponent("approval-code"), encoding: .utf8) { self.approvalCode = code }
                try? await Task.sleep(nanoseconds: 300_000_000)
            }
        }
        defer { monitor.cancel(); approvalCode = nil; busy = false }
        do {
            _ = try await client.call(operation, fields)
            connected = true
            message = "This iPhone is connected."
        } catch { message = error.localizedDescription }
    }

    func importShared(from source: URL? = nil) async {
        guard !busy, let directory = source ?? (try? SharedInbox.directory()),
              let urls = try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil) else { return }
        for url in urls.sorted(by: { $0.lastPathComponent < $1.lastPathComponent }) where url.pathExtension == "json" {
            do {
                let copy = try JSONDecoder().decode(SharedCopy.self, from: Data(contentsOf: url))
                var fields: [String: Any] = ["name": device]
                if let identifier = UUID(uuidString: copy.id) {
                    var bytes = identifier.uuid
                    fields["id"] = withUnsafeBytes(of: &bytes) { Self.base64URL(Data($0)) }
                }
                if copy.kind == "image", let data = copy.data {
                    fields.merge(["kind": "image", "mime": copy.mime ?? "image/png", "data": Self.base64URL(data)]) { _, new in new }
                } else if let text = copy.text { fields["text"] = text }
                _ = try await client.call("save", fields)
                try FileManager.default.removeItem(at: url)
            } catch { message = "Couldn't import a shared item: \(error.localizedDescription)" }
        }
        await load()
    }

    static func base64URL(_ data: Data) -> String {
        data.base64EncodedString().replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "=", with: "")
    }
    static func decodeBase64URL(_ text: String) -> Data? {
        var text = text.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        text += String(repeating: "=", count: (4 - text.count % 4) % 4)
        return Data(base64Encoded: text)
    }
}

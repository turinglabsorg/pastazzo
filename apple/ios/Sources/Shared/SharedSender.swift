import Foundation

enum SharedSender {
    enum Outcome: Equatable {
        case sent, queued, savedLocally

        var title: String { self == .sent ? "Sent securely" : "Saved on this iPhone" }
        var message: String {
            switch self {
            case .sent: return "Your encrypted copy is ready for your connected devices."
            case .queued: return "The encrypted copy is waiting to sync. Retry now, or Pastazzo will retry when you next open it."
            case .savedLocally: return "Open Pastazzo once to finish setup or resume your connection. Your copy is saved."
            }
        }
    }

    static func send(_ copy: SharedCopy, client: MobileClient? = nil, inbox: URL? = nil) async throws -> Outcome {
        try SharedInbox.save(copy, to: inbox)
        do {
            let client = try client ?? MobileClient(root: SharedStorage.extensionRoot())
            guard let identifier = UUID(uuidString: copy.id) else { throw MobileClient.error("Invalid shared item identifier.") }
            var bytes = identifier.uuid
            let id = withUnsafeBytes(of: &bytes) {
                Data($0).base64EncodedString().replacingOccurrences(of: "+", with: "-")
                    .replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "=", with: "")
            }
            var fields: [String: Any] = ["id": id]
            let image = client.root.appendingPathComponent("share-input", isDirectory: true).appendingPathComponent("\(id).png")
            if copy.kind == "image", let data = copy.data {
                try FileManager.default.createDirectory(at: image.deletingLastPathComponent(), withIntermediateDirectories: true,
                    attributes: [.posixPermissions: 0o700, .protectionKey: FileProtectionType.complete])
                try data.write(to: image, options: [.atomic, .completeFileProtection])
                try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: image.path)
                fields["kind"] = "image"
            } else if let text = copy.text { fields["text"] = text }
            defer { try? FileManager.default.removeItem(at: image) }
            let response = try await client.call("share", fields)
            let directory = try inbox ?? SharedInbox.directory()
            try? FileManager.default.removeItem(at: directory.appendingPathComponent("\(copy.id).json"))
            return response["queued"] as? Bool == true ? .queued : .sent
        } catch {
            return .savedLocally
        }
    }
}

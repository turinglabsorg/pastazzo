import Foundation
import PastazzoMobile

final class MobileClient: @unchecked Sendable {
    let root: URL
    private let queue = DispatchQueue(label: "org.pastazzo.mobile")

    init(root: URL? = nil) throws {
        self.root = try root ?? FileManager.default.url(for: .applicationSupportDirectory,
            in: .userDomainMask, appropriateFor: nil, create: true).appendingPathComponent("Pastazzo", isDirectory: true)
        try FileManager.default.createDirectory(at: self.root, withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700, .protectionKey: FileProtectionType.complete])
        var protected = self.root
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try protected.setResourceValues(values)
    }

    func call(_ operation: String, _ fields: [String: Any] = [:]) async throws -> [String: Any] {
        try await withCheckedThrowingContinuation { continuation in
            queue.async {
                do {
                    var request = fields
                    request["operation"] = operation
                    request["root"] = self.root.path
                    var input = try JSONSerialization.data(withJSONObject: request)
                    defer { input.resetBytes(in: 0..<input.count) }
                    let output = input.withUnsafeBytes { bytes in
                        pastazzo_mobile_call(bytes.bindMemory(to: UInt8.self).baseAddress, input.count)
                    }
                    guard let output else { throw Self.error("The sync client returned no answer.") }
                    defer { pastazzo_mobile_free(output) }
                    let response = try JSONSerialization.jsonObject(with: Data(String(cString: output).utf8)) as? [String: Any]
                    guard response?["ok"] as? Bool == true else {
                        throw Self.error(response?["error"] as? String ?? "The operation failed.")
                    }
                    continuation.resume(returning: response?["result"] as? [String: Any] ?? [:])
                } catch { continuation.resume(throwing: error) }
            }
        }
    }

    static func error(_ message: String) -> Error {
        NSError(domain: "Pastazzo", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
    }
}

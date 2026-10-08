import Foundation

struct SharedCopy: Codable {
    let id: String
    let kind: String
    let text: String?
    let data: Data?
    let mime: String?
}

enum SharedInbox {
    static let group = "group.org.pastazzo.clipboard"

    static func directory() throws -> URL {
        guard let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group) else {
            throw NSError(domain: "Pastazzo", code: 1, userInfo: [NSLocalizedDescriptionKey: "The shared container is unavailable. Enable the Pastazzo App Group in signing settings."])
        }
        let directory = container.appendingPathComponent("Inbox", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700, .protectionKey: FileProtectionType.complete])
        var protected = directory
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try protected.setResourceValues(values)
        return directory
    }

    static func save(_ copy: SharedCopy, to destination: URL? = nil) throws {
        guard UUID(uuidString: copy.id) != nil else { throw NSError(domain: "Pastazzo", code: 3, userInfo: [NSLocalizedDescriptionKey: "Invalid shared item identifier."]) }
        if let text = copy.text, text.utf8.count > 1024 * 1024 { throw tooLarge }
        if let data = copy.data, data.count > 25 * 1024 * 1024 { throw tooLarge }
        let inbox = try destination ?? directory()
        try FileManager.default.createDirectory(at: inbox, withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700, .protectionKey: FileProtectionType.complete])
        let url = inbox.appendingPathComponent("\(copy.id).json")
        try JSONEncoder().encode(copy).write(to: url, options: [.atomic, .completeFileProtection])
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }

    static var tooLarge: Error {
        NSError(domain: "Pastazzo", code: 2, userInfo: [NSLocalizedDescriptionKey: "Text is limited to 1 MB and images to 25 MB."])
    }
}

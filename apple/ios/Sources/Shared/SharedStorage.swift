import Foundation
import Darwin

final class StorageLock {
    private let descriptor: Int32

    init(_ url: URL, timeout: TimeInterval = 3) throws {
        let opened = Darwin.open(url.path, O_CREAT | O_RDWR | O_NOFOLLOW, 0o600)
        guard opened >= 0 else { throw MobileClient.error("Pastazzo couldn't lock its storage.") }
        let deadline = ProcessInfo.processInfo.systemUptime + timeout
        while flock(opened, LOCK_EX | LOCK_NB) != 0 {
            guard errno == EWOULDBLOCK || errno == EAGAIN,
                  ProcessInfo.processInfo.systemUptime < deadline else {
                Darwin.close(opened)
                throw MobileClient.error("Pastazzo is busy. Try again shortly.")
            }
            usleep(20_000)
        }
        descriptor = opened
    }

    deinit {
        flock(descriptor, LOCK_UN)
        Darwin.close(descriptor)
    }
}

enum SharedStorage {
    static func container() throws -> URL {
        guard let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: SharedInbox.group) else {
            throw MobileClient.error("Pastazzo's shared storage is unavailable.")
        }
        return container
    }

    static func appRoot() throws -> URL {
        let container = try container()
        let legacy = try FileManager.default.url(for: .applicationSupportDirectory,
            in: .userDomainMask, appropriateFor: nil, create: true).appendingPathComponent("Pastazzo", isDirectory: true)
        return try prepare(legacy: legacy, container: container)
    }

    static func extensionRoot() throws -> URL {
        let root = try container().appendingPathComponent("Pastazzo", isDirectory: true)
        guard FileManager.default.fileExists(atPath: root.appendingPathComponent(".shared-ready").path) else {
            throw MobileClient.error("Open Pastazzo once to enable direct sharing on this iPhone.")
        }
        return root
    }

    static func prepare(legacy: URL, container: URL) throws -> URL {
        let lock = try StorageLock(container.appendingPathComponent(".migration.lock"))
        defer { withExtendedLifetime(lock) {} }
        let files = FileManager.default
        let root = container.appendingPathComponent("Pastazzo", isDirectory: true)
        let marker = root.appendingPathComponent(".shared-ready")
        if !files.fileExists(atPath: marker.path) {
            if files.fileExists(atPath: legacy.path) {
                guard !files.fileExists(atPath: root.path) else {
                    throw MobileClient.error("Pastazzo found two storage folders. Your copies are preserved; migration needs attention.")
                }
                try files.moveItem(at: legacy, to: root)
            }
            try files.createDirectory(at: root, withIntermediateDirectories: true,
                attributes: [.posixPermissions: 0o700, .protectionKey: FileProtectionType.complete])
            try Data("1".utf8).write(to: marker, options: [.atomic, .completeFileProtection])
            try files.setAttributes([.posixPermissions: 0o600], ofItemAtPath: marker.path)
        }
        try files.setAttributes([.posixPermissions: 0o700, .protectionKey: FileProtectionType.complete], ofItemAtPath: root.path)
        var protected = root
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try protected.setResourceValues(values)
        return root
    }
}

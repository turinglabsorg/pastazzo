import XCTest
@testable import Pastazzo

final class SharedStorageTests: XCTestCase {
    private func temporary() throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return root
    }

    func testMigrationPreservesHistoryAndPendingFilesAndRunsOnlyOnce() throws {
        let directory = try temporary()
        defer { try? FileManager.default.removeItem(at: directory) }
        let legacy = directory.appendingPathComponent("Legacy")
        let container = directory.appendingPathComponent("Group")
        try FileManager.default.createDirectory(at: legacy, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: container, withIntermediateDirectories: true)
        for name in ["history.json", "outbox.sealed", "sync-metadata.json"] {
            try Data("Synthetic \(name)".utf8).write(to: legacy.appendingPathComponent(name))
        }
        let migrated = try SharedStorage.prepare(legacy: legacy, container: container)
        XCTAssertFalse(FileManager.default.fileExists(atPath: legacy.path))
        for name in ["history.json", "outbox.sealed", "sync-metadata.json"] {
            XCTAssertEqual(try Data(contentsOf: migrated.appendingPathComponent(name)), Data("Synthetic \(name)".utf8))
        }
        XCTAssertEqual(try FileManager.default.attributesOfItem(atPath: migrated.path)[.posixPermissions] as? Int, 0o700)
        XCTAssertEqual(try SharedStorage.prepare(legacy: legacy, container: container), migrated)
    }

    func testInterruptedMigrationFinishesAfterTheAtomicMove() throws {
        let directory = try temporary()
        defer { try? FileManager.default.removeItem(at: directory) }
        let root = directory.appendingPathComponent("Pastazzo", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try Data("Saved before interruption".utf8).write(to: root.appendingPathComponent("history.json"))
        XCTAssertEqual(try SharedStorage.prepare(legacy: directory.appendingPathComponent("Gone"), container: directory), root)
        XCTAssertTrue(FileManager.default.fileExists(atPath: root.appendingPathComponent(".shared-ready").path))
        XCTAssertEqual(try String(contentsOf: root.appendingPathComponent("history.json"), encoding: .utf8), "Saved before interruption")
    }

    func testConflictingFoldersArePreservedWithoutOverwriting() throws {
        let directory = try temporary()
        defer { try? FileManager.default.removeItem(at: directory) }
        let legacy = directory.appendingPathComponent("Legacy")
        let shared = directory.appendingPathComponent("Pastazzo")
        for root in [legacy, shared] { try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true) }
        XCTAssertThrowsError(try SharedStorage.prepare(legacy: legacy, container: directory))
        XCTAssertTrue(FileManager.default.fileExists(atPath: legacy.path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: shared.path))
    }

    func testIndependentClientsRespectTheSameStorageLock() async throws {
        let root = try temporary()
        defer { try? FileManager.default.removeItem(at: root) }
        let url = root.appendingPathComponent(".client.lock")
        var held: StorageLock? = try StorageLock(url)
        XCTAssertNotNil(held)
        XCTAssertThrowsError(try StorageLock(url, timeout: 0.05))
        held = nil
        try await withThrowingTaskGroup(of: Void.self) { tasks in
            for index in 0..<12 {
                tasks.addTask {
                    let client = try MobileClient(root: root)
                    _ = try await client.call("save", ["name": "Shared iPhone", "text": "Concurrent copy \(index)"])
                }
            }
            try await tasks.waitForAll()
        }
        let history = try await MobileClient(root: root).call("history")
        XCTAssertEqual((history["items"] as? [Any])?.count, 12)
    }

    func testUnconnectedShareKeepsProtectedInboxForTheApp() async throws {
        let root = try temporary()
        defer { try? FileManager.default.removeItem(at: root) }
        let inbox = root.appendingPathComponent("Inbox")
        let copy = SharedCopy(id: UUID().uuidString, kind: "text", text: "Keep this until connected", data: nil, mime: nil)
        let result = try await SharedSender.send(copy, client: MobileClient(root: root.appendingPathComponent("App")), inbox: inbox)
        XCTAssertEqual(result, .savedLocally)
        let path = inbox.appendingPathComponent("\(copy.id).json")
        XCTAssertTrue(FileManager.default.fileExists(atPath: path.path))
        XCTAssertEqual(try FileManager.default.attributesOfItem(atPath: path.path)[.posixPermissions] as? Int, 0o600)
    }
}

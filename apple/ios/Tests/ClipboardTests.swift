import XCTest
@testable import Pastazzo

final class ClipboardTests: XCTestCase {
    @MainActor
    func testWidgetURLsReachTheAppSceneAndOnlyPasteImports() async throws {
        #if targetEnvironment(simulator)
        let client = try MobileClient()
        let status = try await client.call("status")
        guard status["logged_in"] as? Bool == false else { throw XCTSkip("Use an unconnected simulator for scene routing tests") }
        let fixture = "Widget scene routing \(UUID().uuidString)"
        UIPasteboard.general.string = fixture
        let openedHistory = await UIApplication.shared.open(ClipboardAction.history.url, options: [:])
        XCTAssertTrue(openedHistory)
        try await Task.sleep(nanoseconds: 200_000_000)
        let before = try await client.call("history")
        XCTAssertFalse((before["items"] as? [[String: Any]] ?? []).contains { $0["preview"] as? String == fixture })
        let openedPaste = await UIApplication.shared.open(ClipboardAction.paste.url, options: [:])
        XCTAssertTrue(openedPaste)
        var saved = false
        for _ in 0..<100 {
            let history = try await client.call("history")
            let matches = (history["items"] as? [[String: Any]] ?? []).filter { $0["preview"] as? String == fixture }
            if !matches.isEmpty { XCTAssertEqual(matches.count, 1); saved = true; break }
            try await Task.sleep(nanoseconds: 50_000_000)
        }
        XCTAssertTrue(saved, "The app scene must handle the widget URL and save its clipboard")
        XCTAssertEqual(UIPasteboard.general.string, fixture)
        #else
        throw XCTSkip("Scene routing uses a simulator with an unconnected test profile")
        #endif
    }

    func testWidgetLinksMatchOnlyTheTwoExplicitActions() throws {
        for action in ClipboardAction.allCases {
            XCTAssertEqual(ClipboardAction(url: action.url), action)
        }
        for link in ["https://paste", "pastazzo://pair", "pastazzo://paste?text=external",
            "pastazzo://user@paste", "pastazzo://paste:443", "pastazzo://history/private", "pastazzo://paste#other"] {
            XCTAssertNil(ClipboardAction(url: try XCTUnwrap(URL(string: link))))
        }
    }

    @MainActor
    func testWidgetPasteImportsTextAndImagesWithoutChangingTheClipboard() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let model = HistoryModel(client: try MobileClient(root: root))
        UIPasteboard.general.string = "Explicit widget paste"
        await model.load(sync: false)
        XCTAssertTrue(model.items.isEmpty, "Opening history must not import the clipboard")
        await model.pasteFromClipboard()
        XCTAssertEqual(model.items.count, 1)
        XCTAssertEqual(model.items[0].preview, "Explicit widget paste")
        XCTAssertEqual(UIPasteboard.general.string, "Explicit widget paste")
        await model.pasteFromClipboard()
        XCTAssertEqual(model.items.count, 1, "Repeating Paste must not add another identical card")
        let image = UIGraphicsImageRenderer(size: CGSize(width: 8, height: 8)).image { context in
            UIColor.orange.setFill(); context.fill(CGRect(x: 0, y: 0, width: 8, height: 8))
        }
        UIPasteboard.general.image = image
        await model.pasteFromClipboard()
        XCTAssertEqual(model.items.count, 2)
        XCTAssertTrue(model.items.contains { $0.kind == "image" })
        XCTAssertEqual(UIPasteboard.general.image?.size, image.size)
        await model.pasteFromClipboard()
        XCTAssertEqual(model.items.count, 2, "Repeated image Paste must show one image card")
        UIPasteboard.general.items = []
        await model.pasteFromClipboard()
        XCTAssertEqual(model.items.count, 2)
        XCTAssertNotNil(model.message)
    }

    func testNativeBridgePersistsTextAcrossClientInstancesAndRejectsTraversal() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let client = try MobileClient(root: root)
        let status = try await client.call("status")
        XCTAssertEqual(status["logged_in"] as? Bool, false)
        _ = try await client.call("save", ["text": "Native iOS clipboard integration", "name": "Test iPhone"])
        let restarted = try MobileClient(root: root)
        let history = try await restarted.call("history")
        let items = try XCTUnwrap(history["items"] as? [[String: Any]])
        XCTAssertEqual(items.count, 1)
        XCTAssertEqual(items[0]["origin"] as? String, "Test iPhone")
        let id = try XCTUnwrap(items[0]["id"] as? String)
        let item = try await restarted.call("item", ["id": id])
        XCTAssertEqual((item["item"] as? [String: Any])?["text"] as? String, "Native iOS clipboard integration")
        do { _ = try await restarted.call("item", ["id": "../../sync"]); XCTFail("Traversal must fail") }
        catch {}
        _ = try await restarted.call("clear_local")
        let empty = try await restarted.call("history")
        XCTAssertEqual((empty["items"] as? [Any])?.count, 0)
    }

    @MainActor
    func testNativeHistoryDeduplicatesFullContentAcrossRestarts() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let client = try MobileClient(root: root)
        let prefix = String(repeating: "🍊", count: 400)
        let first = prefix + " first ending"
        let newestId = HistoryModel.base64URL(Data(repeating: 2, count: 16))
        let fields: [String: Any] = ["text": first, "name": "MacBook", "id": HistoryModel.base64URL(Data(repeating: 1, count: 16))]
        _ = try await client.call("save", fields)
        _ = try await client.call("save", ["text": first, "name": "Mac Pro", "id": newestId])
        _ = try await client.call("save", ["text": prefix + " second ending", "name": "MacBook"])
        let restarted = try MobileClient(root: root)
        let history = try await restarted.call("history")
        let items = try XCTUnwrap(history["items"] as? [[String: Any]])
        XCTAssertEqual(items.count, 2, "Different full text must stay separate despite identical previews")
        let newestCopy = try XCTUnwrap(items.first { $0["id"] as? String == newestId })
        XCTAssertEqual(newestCopy["origin"] as? String, "Mac Pro")
        let copy = try await restarted.call("item", ["id": try XCTUnwrap(newestCopy["id"] as? String)])
        XCTAssertEqual((copy["item"] as? [String: Any])?["text"] as? String, first)
    }

    @MainActor
    func testShareInboxImportsOnceAfterRestart() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let copy = SharedCopy(id: UUID().uuidString, kind: "text", text: "Shared explicitly", data: nil, mime: nil)
        let inbox = root.appendingPathComponent("Shared", isDirectory: true)
        let url = inbox.appendingPathComponent("\(copy.id).json")
        defer { try? FileManager.default.removeItem(at: url) }
        try SharedInbox.save(copy, to: inbox)
        let permissions = try FileManager.default.attributesOfItem(atPath: url.path)[.posixPermissions] as? Int
        XCTAssertEqual(permissions, 0o600)
        let model = HistoryModel(client: try MobileClient(root: root))
        await model.importShared(from: inbox)
        XCTAssertFalse(FileManager.default.fileExists(atPath: url.path))
        XCTAssertEqual(model.items.count, 1)
        try SharedInbox.save(copy, to: inbox)
        let restarted = HistoryModel(client: try MobileClient(root: root))
        await restarted.importShared(from: inbox)
        XCTAssertEqual(restarted.items.count, 1)
        XCTAssertEqual(restarted.items[0].preview, "Shared explicitly")
    }

    @MainActor
    func testCopyOnlyWritesClipboardWhenExplicitlyRequested() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let model = HistoryModel(client: try MobileClient(root: root))
        UIPasteboard.general.string = "Unchanged until Copy"
        await model.save(text: "Copy on demand")
        await model.load(sync: false)
        XCTAssertEqual(UIPasteboard.general.string, "Unchanged until Copy")
        let item = try XCTUnwrap(model.items.first)
        await model.copy(item)
        XCTAssertEqual(UIPasteboard.general.string, "Copy on demand")
    }
}

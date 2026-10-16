import XCTest
@testable import Pastazzo

final class HistoryLoadingTests: XCTestCase {
    @MainActor
    func testSavedCopiesAppearBeforeNetworkRefreshFinishes() async {
        let client = HistoryLoadingClient()
        let model = HistoryModel(client: client)
        let load = Task { await model.load() }
        await client.waitForRefresh()
        XCTAssertEqual(model.items.map(\.id), ["cached"])
        XCTAssertTrue(model.busy)
        await client.finishRefresh()
        await load.value
        XCTAssertEqual(model.items.map(\.id), ["fresh"])
        XCTAssertFalse(model.busy)
    }

    @MainActor
    func testUnavailableConnectionDoesNotHideSavedCopies() async {
        let model = HistoryModel(client: HistoryLoadingClient(statusFails: true))
        await model.load()
        XCTAssertEqual(model.items.map(\.id), ["cached"])
        XCTAssertEqual(model.message, "Connection unavailable")
        XCTAssertFalse(model.busy)
    }

    @MainActor
    func testFailedRefreshKeepsSavedCopiesAndReportsTheFailure() async {
        let client = HistoryLoadingClient()
        let model = HistoryModel(client: client)
        let load = Task { await model.load() }
        await client.waitForRefresh()
        await client.finishRefresh(failing: true)
        await load.value
        XCTAssertEqual(model.items.map(\.id), ["cached"])
        XCTAssertEqual(model.message, "Couldn't sync: Connection unavailable")
        XCTAssertFalse(model.busy)
    }

    @MainActor
    func testLocalLoadDoesNotStartNetworkRefresh() async {
        let client = HistoryLoadingClient()
        let model = HistoryModel(client: client)
        await model.load(sync: false)
        XCTAssertEqual(model.items.map(\.id), ["cached"])
        let refreshCount = await client.refreshCount
        XCTAssertEqual(refreshCount, 0)
        XCTAssertFalse(model.busy)
    }
}

private actor HistoryLoadingClient: MobileClientCalling {
    let root = URL(fileURLWithPath: "/synthetic-pastazzo-history-test")
    private let statusFails: Bool
    private var refreshed = false
    private var completion: CheckedContinuation<Void, Error>?
    private var waiting: [CheckedContinuation<Void, Never>] = []
    private(set) var refreshCount = 0

    init(statusFails: Bool = false) { self.statusFails = statusFails }

    func call(_ operation: String, _ fields: [String: Any]) async throws -> [String: Any] {
        switch operation {
        case "history":
            return ["items": [["id": refreshed ? "fresh" : "cached", "created_at": 1,
                "origin": "Synthetic Mac", "kind": "text", "preview": "Synthetic saved copy",
                "size": 20, "queued": false]]]
        case "status":
            if statusFails { throw Failure.unavailable }
            return ["logged_in": true, "device_name": "Synthetic iPhone"]
        case "refresh":
            try await withCheckedThrowingContinuation { continuation in
                completion = continuation
                refreshCount += 1
                waiting.forEach { $0.resume() }
                waiting.removeAll()
            }
            refreshed = true
            return [:]
        default:
            throw Failure.unavailable
        }
    }

    func waitForRefresh() async {
        guard refreshCount == 0 else { return }
        await withCheckedContinuation { waiting.append($0) }
    }

    func finishRefresh(failing: Bool = false) {
        if failing { completion?.resume(throwing: Failure.unavailable) }
        else { completion?.resume() }
        completion = nil
    }

    private enum Failure: LocalizedError {
        case unavailable
        var errorDescription: String? { "Connection unavailable" }
    }
}

import XCTest
@testable import Pastazzo

final class ShareSyncTests: XCTestCase {
    private let control = URL(string: "http://127.0.0.1:32952")!

    private func fixture(_ path: String, body: [String: Any]? = nil) async throws -> [String: Any] {
        var request = URLRequest(url: control.appendingPathComponent(path))
        request.timeoutInterval = 5
        if let body {
            request.httpMethod = "POST"
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONSerialization.data(withJSONObject: body)
        }
        let (data, response) = try await URLSession.shared.data(for: request)
        XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
        return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }

    @MainActor
    func testDirectShareUsesProtectedConnectionAndRetriesOfflineWithoutDuplicates() async throws {
        #if targetEnvironment(simulator)
        guard UserDefaults.standard.bool(forKey: "PastazzoSyntheticShareFixture") else {
            throw XCTSkip("Opt in on an isolated simulator with the synthetic loopback fixture running")
        }
        let client = try MobileClient()
        let status = try await client.call("status")
        if status["logged_in"] as? Bool == true,
           status["server_url"] as? String != "http://127.0.0.1:32951" {
            throw XCTSkip("Refusing to use an existing personal connection for fixture testing")
        }
        if status["logged_in"] as? Bool != true {
            let response = try await fixture("link")
            let link = try XCTUnwrap(response["link"] as? String)
            let fields = try XCTUnwrap(URLComponents(string: link)?.queryItems)
            let encoded = try XCTUnwrap(fields.first { $0.name == "server" }?.value)
            let server = try XCTUnwrap(HistoryModel.decodeBase64URL(encoded))
            guard String(data: server, encoding: .utf8) == "http://127.0.0.1:32951" else {
                throw XCTSkip("Refusing a pairing link outside the synthetic loopback fixture")
            }
            _ = try await client.call("pair", ["link": link, "name": "Share iPhone QA"])
        }
        let connected = try await client.call("status")
        XCTAssertEqual(connected["server_url"] as? String, "http://127.0.0.1:32951")
        let incoming = try SharedStorage.extensionRoot()
        XCTAssertEqual(incoming, client.root)
        let image = UIGraphicsImageRenderer(size: CGSize(width: 24, height: 24)).image { context in
            UIColor.orange.setFill(); context.fill(CGRect(x: 0, y: 0, width: 24, height: 24))
        }
        let copy = SharedCopy(id: UUID().uuidString, kind: "image", text: nil, data: try XCTUnwrap(image.pngData()), mime: "image/png")
        let first = try await SharedSender.send(copy)
        XCTAssertEqual(first, .sent)
        let offline = SharedCopy(id: UUID().uuidString, kind: "text", text: "Synthetic offline shared text \(UUID().uuidString)", data: nil, mime: nil)
        _ = try await fixture("offline", body: ["offline": true])
        let queued = try await SharedSender.send(offline, client: client)
        XCTAssertEqual(queued, .queued)
        _ = try await fixture("offline", body: ["offline": false])
        let retried = try await SharedSender.send(offline, client: client)
        XCTAssertEqual(retried, .sent)
        let before = try await fixture("received")
        let again = try await SharedSender.send(offline, client: client)
        XCTAssertEqual(again, .sent)
        let after = try await fixture("received")
        XCTAssertEqual((before["items"] as? [Any])?.count, (after["items"] as? [Any])?.count)
        let values = try XCTUnwrap(after["items"] as? [[String: Any]])
        XCTAssertTrue(values.contains { $0["mime"] as? String == "image/png" && $0["size"] as? Int == copy.data?.count })
        XCTAssertEqual(values.filter { $0["text"] as? String == offline.text }.count, 1)
        #else
        throw XCTSkip("Synthetic share sync is simulator-only")
        #endif
    }

    func testRemoveSyntheticShareProfileAfterManualExtensionVerification() async throws {
        #if targetEnvironment(simulator)
        guard UserDefaults.standard.bool(forKey: "PastazzoCleanUpShareFixture") else {
            throw XCTSkip("Run only after the manual share-extension verification")
        }
        let client = try MobileClient()
        let status = try await client.call("status")
        guard status["server_url"] as? String == "http://127.0.0.1:32951",
              status["username"] as? String == "android-fixture" else {
            throw XCTSkip("Refusing to revoke a profile outside the synthetic fixture")
        }
        _ = try await client.call("logout")
        UserDefaults.standard.removeObject(forKey: "PastazzoSyntheticShareFixture")
        UserDefaults.standard.removeObject(forKey: "PastazzoCleanUpShareFixture")
        #else
        throw XCTSkip("Synthetic profile cleanup is simulator-only")
        #endif
    }
}

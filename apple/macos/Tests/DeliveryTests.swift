import Foundation

@main
struct DeliveryTests {
    static func item(_ delivery: Any? = nil) throws -> HistoryItem {
        var json: [String: Any] = [
            "id": "fixture", "timestamp": 1_791_379_434_274.0, "kind": "text", "mime": "text/plain",
            "preview": "Synthetic clipboard fixture", "text": "Synthetic clipboard fixture", "path": "/tmp/fixture",
        ]
        if let delivery { json["delivery"] = delivery }
        return try JSONDecoder().decode(HistoryItem.self, from: JSONSerialization.data(withJSONObject: json))
    }

    static func status(receivers: [[String: Any]] = [], supported: Bool = true, uploaded: Bool = true) -> [String: Any] {
        ["origin_device": "source", "uploaded_at": uploaded ? 1_791_379_434_275.0 : NSNull(), "supported": supported,
         "peers": [["device_id": "source", "device_name": "MacBook"],
                   ["device_id": "desktop", "device_name": "Mac Pro"],
                   ["device_id": "phone", "device_name": "iPhone"]], "receivers": receivers]
    }

    static func main() throws {
        let legacy = try item()
        precondition(legacy.delivery == nil && legacy.origin.isEmpty && legacy.sourceApp.isEmpty)
        precondition(legacy.copiedAt == legacy.timestamp)

        let pending = try item(status()).delivery!
        precondition(pending.confirmed.isEmpty && pending.summary == "Sent to server")
        precondition(pending.awaiting.map(\.deviceName) == ["iPhone", "Mac Pro"])
        precondition(pending.pendingSummary == "Awaiting: iPhone, Mac Pro")

        let receiver: [String: Any] = ["device_id": "desktop", "device_name": "Mac Pro", "received_at": 1_791_379_434_300.0]
        let source: [String: Any] = ["device_id": "source", "device_name": "MacBook", "received_at": 1_791_379_434_300.0]
        let confirmed = try item(status(receivers: [receiver, receiver, source])).delivery!
        precondition(confirmed.confirmed.count == 1 && confirmed.summary == "Received on Mac Pro")
        precondition(confirmed.awaiting.map(\.deviceName) == ["iPhone"])
        precondition(confirmed.detail.contains("Mac Pro:") && confirmed.detail.contains("Awaiting: iPhone"))
        precondition(confirmed.confirmed[0].date.timeIntervalSince1970 == 1_791_379_434.300)

        let unsupported = try item(status(supported: false)).delivery!
        precondition(unsupported.pendingSummary == "Device confirmations unavailable")
        precondition(unsupported.confirmed.isEmpty)
        let queued = try item(status(uploaded: false)).delivery!
        precondition(queued.summary == "Queued for sync")

        let invalid = try item(["receivers": "invalid metadata"])
        precondition(invalid.delivery == nil && invalid.preview == "Synthetic clipboard fixture")
        print("PASS: legacy history, upload vs receipt, pending peers, receipt deduplication, receipt dates, unavailable support, offline queue, malformed metadata")
    }
}

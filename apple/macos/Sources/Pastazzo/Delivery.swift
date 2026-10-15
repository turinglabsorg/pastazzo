import Foundation

struct DeliveryStatus: Decodable, Equatable {
    struct Peer: Decodable, Equatable {
        let deviceId: String
        let deviceName: String

        private enum CodingKeys: String, CodingKey {
            case deviceId = "device_id"
            case deviceName = "device_name"
        }
    }

    struct Receiver: Decodable, Equatable {
        let deviceId: String
        let deviceName: String
        let receivedAt: Double

        private enum CodingKeys: String, CodingKey {
            case deviceId = "device_id"
            case deviceName = "device_name"
            case receivedAt = "received_at"
        }

        var name: String { deviceName.isEmpty ? "Another device" : deviceName }
        var date: Date { Date(timeIntervalSince1970: receivedAt / 1000) }
    }

    let originDevice: String
    let uploadedAt: Double?
    let supported: Bool
    let peers: [Peer]
    let receivers: [Receiver]

    private enum CodingKeys: String, CodingKey {
        case originDevice = "origin_device"
        case uploadedAt = "uploaded_at"
        case supported, peers, receivers
    }

    var confirmed: [Receiver] {
        var seen = Set<String>()
        return receivers.filter {
            !$0.deviceId.isEmpty && $0.deviceId != originDevice && seen.insert($0.deviceId).inserted
        }.sorted {
            $0.receivedAt == $1.receivedAt ? $0.deviceId < $1.deviceId : $0.receivedAt < $1.receivedAt
        }
    }

    var awaiting: [Peer] {
        let received = Set(confirmed.map(\.deviceId))
        var seen = Set<String>()
        return peers.filter {
            !$0.deviceId.isEmpty && $0.deviceId != originDevice && !received.contains($0.deviceId)
                && seen.insert($0.deviceId).inserted
        }.sorted { $0.deviceName.localizedStandardCompare($1.deviceName) == .orderedAscending }
    }

    var summary: String {
        if !confirmed.isEmpty { return "Received on " + confirmed.map(\.name).joined(separator: ", ") }
        return uploadedAt == nil ? "Queued for sync" : "Sent to server"
    }

    var pendingSummary: String? {
        if !supported { return "Device confirmations unavailable" }
        guard !awaiting.isEmpty else { return nil }
        return "Awaiting: " + awaiting.map { $0.deviceName.isEmpty ? "Another device" : $0.deviceName }.joined(separator: ", ")
    }

    var detail: String {
        var lines = confirmed.map { "\($0.name): \(Self.dateFormatter.string(from: $0.date))" }
        if let pendingSummary { lines.append(pendingSummary) }
        if lines.isEmpty { lines.append("No receiving device has confirmed this item.") }
        lines.append("A confirmation means the app saved this item on that device.")
        return lines.joined(separator: "\n")
    }

    private static let dateFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateStyle = .medium
        formatter.timeStyle = .medium
        return formatter
    }()
}

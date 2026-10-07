import AppKit
import Foundation

/// What `pastazzo-sync` reports in `<data>/sync/`: this device's name, and the
/// transfers in progress.
final class SyncMonitor: ObservableObject {
    /// "↓ Mac Pro 45% of 3.4 MB", or nil when nothing is moving.
    @Published private(set) var transferText: String?
    /// Just the percentage, for the menu bar.
    @Published private(set) var transferShort: String?
    @Published private(set) var localDevice = ""

    private let directory: URL
    private var timer: Timer?

    /// A transfers.json not updated for this long belongs to a daemon that stopped.
    private static let staleAfter: TimeInterval = 60

    init(dataDirectory: URL) {
        directory = dataDirectory.appendingPathComponent("sync")
    }

    func start() {
        timer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
            self?.read()
        }
        read()
    }

    private struct Info: Decodable {
        let deviceName: String
    }

    private struct Transfers: Decodable {
        let updated: Double
        let transfers: [Transfer]
    }

    struct Transfer: Decodable {
        let direction: String
        let device: String
        let size: Double
        let done: Double

        var percent: Int { size > 0 ? Int(100 * done / size) : 0 }
        var arrow: String { direction == "send" ? "↑" : "↓" }
    }

    private func decode<T: Decodable>(_ type: T.Type, _ name: String) -> T? {
        guard let data = try? Data(contentsOf: directory.appendingPathComponent(name)) else { return nil }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try? decoder.decode(type, from: data)
    }

    private func read() {
        let device = decode(Info.self, "status.json")?.deviceName ?? ""
        if device != localDevice {
            localDevice = device
        }
        var transfers: [Transfer] = []
        if let file = decode(Transfers.self, "transfers.json"),
           Date().timeIntervalSince1970 - file.updated / 1000 < Self.staleAfter {
            transfers = file.transfers
        }
        let text = transfers.isEmpty ? nil : transfers.map(Self.describe).joined(separator: "   ")
        let short = transfers.first.map { "\($0.arrow) \($0.percent)%" }
        if text != transferText {
            transferText = text
        }
        if short != transferShort {
            transferShort = short
        }
    }

    static func describe(_ transfer: Transfer) -> String {
        let who = transfer.direction == "send" ? "Sending" : (transfer.device.isEmpty ? "Receiving" : transfer.device)
        let size = ByteCountFormatter.string(fromByteCount: Int64(transfer.size), countStyle: .file)
        return "\(transfer.arrow) \(who) \(transfer.percent)% of \(size)"
    }
}

/// `pastazzo-sync status --json`, as the settings show it.
struct SyncStatus: Decodable {
    let loggedIn: Bool
    let error: String?
    let serverUrl: String?
    let username: String?
    let serverFingerprint: String?
    let accountKeyFingerprint: String?
    let thisDevice: SyncDevice?
    let devices: [SyncDevice]?
    let devicesError: String?
}

struct SyncDevice: Decodable, Identifiable, Equatable {
    let id: String
    let name: String
    let fingerprint: String
    let this: Bool?

    var isThisDevice: Bool { this ?? false }
}

/// Runs `pastazzo-sync` commands for the settings.
final class SyncClient {
    let cli = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".local/bin/pastazzo-sync")
    private let queue = DispatchQueue(label: "org.pastazzo.sync-client")

    func status(completion: @escaping (SyncStatus) -> Void) {
        run(["status", "--json"]) { output, error in
            let decoder = JSONDecoder()
            decoder.keyDecodingStrategy = .convertFromSnakeCase
            let status = output.flatMap { try? decoder.decode(SyncStatus.self, from: $0) }
                ?? SyncStatus(loggedIn: false, error: error ?? "unexpected answer from pastazzo-sync", serverUrl: nil,
                              username: nil, serverFingerprint: nil, accountKeyFingerprint: nil, thisDevice: nil,
                              devices: nil, devicesError: nil)
            completion(status)
        }
    }

    /// Runs a command; the completion gets nil on success, else the error.
    func perform(_ arguments: [String], completion: @escaping (String?) -> Void) {
        run(arguments) { _, error in completion(error) }
    }

    private func run(_ arguments: [String], completion: @escaping (Data?, String?) -> Void) {
        queue.async {
            let process = Process()
            process.executableURL = self.cli
            process.arguments = arguments
            let stdout = Pipe()
            let stderr = Pipe()
            process.standardOutput = stdout
            process.standardError = stderr
            process.standardInput = FileHandle.nullDevice
            do {
                try process.run()
            } catch {
                DispatchQueue.main.async { completion(nil, "\(self.cli.path) is missing: install pastazzo-sync") }
                return
            }
            let output = stdout.fileHandleForReading.readDataToEndOfFile()
            let errorOutput = stderr.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            let error = process.terminationStatus == 0
                ? nil
                : (String(data: errorOutput, encoding: .utf8) ?? "failed")
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                    .replacingOccurrences(of: "pastazzo-sync: ", with: "")
            DispatchQueue.main.async { completion(output, error) }
        }
    }
}

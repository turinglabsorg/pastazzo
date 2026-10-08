import AppKit
import Foundation

/// What `pastazzo-sync` reports in `<data>/sync/`: this device's name, and the
/// transfers in progress.
final class SyncMonitor: ObservableObject {
    /// "↓ Mac Pro 45% of 3.4 MB", or nil when nothing is moving.
    @Published private(set) var transferText: String?
    /// Just the percentage, for the menu bar.
    @Published private(set) var transferShort: String?
    @Published private(set) var localDevice = Host.current().localizedName ?? "This Mac"
    /// Devices that logged in and wait for approval.
    @Published private(set) var waitingDevices = 0

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

    private struct Approvals: Decodable {
        struct Waiting: Decodable {
            let id: String
            let code: String
        }

        let updated: Double
        let pending: [Waiting]
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
        let device = decode(Info.self, "status.json")?.deviceName ?? Host.current().localizedName ?? "This Mac"
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
        var waiting = 0
        if let approvals = decode(Approvals.self, "approvals.json"),
           Date().timeIntervalSince1970 - approvals.updated / 1000 < Self.staleAfter {
            waiting = approvals.pending.count
        }
        if waiting != waitingDevices {
            waitingDevices = waiting
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
    let pendingDevices: [SyncPending]?
    let canApprove: Bool?
    let devicesError: String?
}

/// A device waiting for approval.
struct SyncPending: Decodable, Identifiable, Equatable {
    let id: String
    let code: String
}

struct SyncDevice: Decodable, Identifiable, Equatable {
    let id: String
    let name: String
    let fingerprint: String
    let this: Bool?

    var isThisDevice: Bool { this ?? false }
}

struct SyncPairingStatus: Decodable {
    let expiresAt: Double
    let name: String?
    let code: String?
    let completed: Bool
}

/// Runs `pastazzo-sync` commands for the settings.
final class SyncClient {
    let cli = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".local/bin/pastazzo-sync")
    private let queue = DispatchQueue(label: "org.pastazzo.sync-client")

    func createPairing(completion: @escaping (String?, Double?, String?) -> Void) {
        run(["pair", "create", "--json"]) { data, error in
            guard let data, let result = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let link = result["link"] as? String, let expiry = result["expires_at"] as? Double else {
                completion(nil, nil, error ?? "Couldn't create the pairing QR."); return
            }
            completion(link, expiry, nil)
        }
    }

    func pairingStatus(completion: @escaping (SyncPairingStatus?, String?) -> Void) {
        run(["pair", "status", "--json"]) { data, error in
            let decoder = JSONDecoder()
            decoder.keyDecodingStrategy = .convertFromSnakeCase
            completion(data.flatMap { try? decoder.decode(SyncPairingStatus.self, from: $0) }, error)
        }
    }

    func login(server: String, fingerprint: String, username: String, password: String,
               onCode: @escaping (String) -> Void, completion: @escaping (String?) -> Void) {
        queue.async {
            let process = Process()
            process.executableURL = self.cli
            process.arguments = ["login", "--server", server, "--fingerprint", fingerprint,
                                 "--username", username, "--password-file", "/dev/stdin"]
            let input = Pipe()
            let output = Pipe()
            let errors = Pipe()
            process.standardInput = input
            process.standardOutput = output
            process.standardError = errors
            do {
                try process.run()
                input.fileHandleForWriting.write(Data(password.utf8))
                try input.fileHandleForWriting.close()
                var pending = ""
                while let data = try output.fileHandleForReading.read(upToCount: 4096), !data.isEmpty {
                    pending += String(decoding: data, as: UTF8.self)
                    while let end = pending.firstIndex(of: "\n") {
                        let line = String(pending[..<end])
                        pending.removeSubrange(...end)
                        if let range = line.range(of: "approval code: ") {
                            let code = String(line[range.upperBound...]).trimmingCharacters(in: .whitespaces)
                            DispatchQueue.main.async { onCode(code) }
                        }
                    }
                }
                let errorData = errors.fileHandleForReading.readDataToEndOfFile()
                process.waitUntilExit()
                let error = process.terminationStatus == 0 ? nil
                    : String(decoding: errorData, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
                DispatchQueue.main.async { completion(error) }
            } catch {
                if process.isRunning { process.terminate() }
                DispatchQueue.main.async { completion(error.localizedDescription) }
            }
        }
    }

    func status(completion: @escaping (SyncStatus) -> Void) {
        run(["status", "--json"]) { output, error in
            let decoder = JSONDecoder()
            decoder.keyDecodingStrategy = .convertFromSnakeCase
            let status = output.flatMap { try? decoder.decode(SyncStatus.self, from: $0) }
                ?? SyncStatus(loggedIn: false, error: error ?? "unexpected answer from pastazzo-sync", serverUrl: nil,
                              username: nil, serverFingerprint: nil, accountKeyFingerprint: nil, thisDevice: nil,
                              devices: nil, pendingDevices: nil, canApprove: nil, devicesError: nil)
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

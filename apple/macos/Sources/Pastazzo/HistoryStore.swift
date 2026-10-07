import Foundation

/// An item of the pastazzo history, as `pastazzo search` prints it.
struct HistoryItem: Decodable, Identifiable, Equatable {
    let id: String
    let timestamp: Double
    let kind: String
    let mime: String
    let preview: String
    let text: String
    let path: String

    var isImage: Bool { kind == "image" }
    var date: Date { Date(timeIntervalSince1970: timestamp / 1000) }
}

/// The pastazzo history, shared with the Linux side: the same `pastazzo` CLI
/// and the same files, which `pastazzo-sync` watches to sync new copies.
final class HistoryStore {
    let cli: URL
    let dataDirectory: URL
    var inboxDirectory: URL { dataDirectory.appendingPathComponent("inbox") }

    /// One at a time and in order, so copies land in the order they were made.
    private let queue = DispatchQueue(label: "org.pastazzo.store")

    init() {
        let home = FileManager.default.homeDirectoryForCurrentUser
        let environment = ProcessInfo.processInfo.environment
        cli = home.appendingPathComponent(".local/bin/pastazzo")
        if let dataHome = environment["XDG_DATA_HOME"], !dataHome.isEmpty {
            dataDirectory = URL(fileURLWithPath: dataHome).appendingPathComponent("pastazzo")
        } else {
            dataDirectory = home.appendingPathComponent(".local/share/pastazzo")
        }
    }

    var isAvailable: Bool { FileManager.default.isExecutableFile(atPath: cli.path) }

    func addText(_ text: String) {
        run(["add"], input: Data(text.utf8)) { _ in }
    }

    func addImage(_ png: Data) {
        run(["add-image", "image/png"], input: png) { _ in }
    }

    /// Moves an item to the top of the history.
    func touch(_ id: String) {
        run(["touch", id], input: nil) { _ in }
    }

    func clear(completion: @escaping () -> Void) {
        run(["clear"], input: nil) { _ in DispatchQueue.main.async(execute: completion) }
    }

    func search(_ query: String, completion: @escaping ([HistoryItem]) -> Void) {
        run(["search", query], input: nil) { output in
            let items = output.flatMap { try? JSONDecoder().decode([HistoryItem].self, from: $0) } ?? []
            DispatchQueue.main.async { completion(items) }
        }
    }

    private func run(_ arguments: [String], input: Data?, completion: @escaping (Data?) -> Void) {
        queue.async {
            let process = Process()
            process.executableURL = self.cli
            process.arguments = arguments
            let stdin = Pipe()
            let stdout = Pipe()
            process.standardInput = stdin
            process.standardOutput = stdout
            process.standardError = FileHandle.nullDevice
            do {
                try process.run()
            } catch {
                NSLog("Pastazzo: can't run \(self.cli.path): \(error)")
                completion(nil)
                return
            }
            if let input = input {
                stdin.fileHandleForWriting.write(input)
            }
            try? stdin.fileHandleForWriting.close()
            let output = stdout.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            completion(process.terminationStatus == 0 ? output : nil)
        }
    }
}

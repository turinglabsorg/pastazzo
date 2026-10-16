import Foundation

protocol MobileClientCalling: Sendable {
    var root: URL { get }
    func call(_ operation: String, _ fields: [String: Any]) async throws -> [String: Any]
}

extension MobileClientCalling {
    func call(_ operation: String) async throws -> [String: Any] {
        try await call(operation, [:])
    }
}

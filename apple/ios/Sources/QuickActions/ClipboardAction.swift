import Foundation

enum ClipboardAction: String, CaseIterable {
    case paste
    case history

    var url: URL { URL(string: "pastazzo://\(rawValue)")! }

    init?(url: URL) {
        guard url.scheme == "pastazzo", url.user == nil, url.password == nil,
              url.port == nil, url.query == nil, url.fragment == nil,
              url.path.isEmpty || url.path == "/", let host = url.host else { return nil }
        self.init(rawValue: host)
    }
}

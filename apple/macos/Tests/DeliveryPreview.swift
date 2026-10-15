import AppKit
import SwiftUI

@main
struct DeliveryPreview {
    static func item(_ index: Int, _ delivery: [String: Any]?, imagePath: String? = nil) throws -> HistoryItem {
        var json: [String: Any] = [
            "id": "fixture-\(index)", "timestamp": Date().timeIntervalSince1970 * 1000,
            "kind": "text", "mime": "text/plain", "origin": "MacBook",
            "text": "Synthetic delivery preview", "preview": "Meeting notes\n\nShare the final design with the team.", "path": "/tmp/fixture",
        ]
        if let delivery { json["delivery"] = delivery }
        if let imagePath { json["kind"] = "image"; json["mime"] = "image/png"; json["path"] = imagePath }
        return try JSONDecoder().decode(HistoryItem.self, from: JSONSerialization.data(withJSONObject: json))
    }

    static func main() throws {
        let app = NSApplication.shared
        app.setActivationPolicy(.prohibited)
        let output = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        let image = NSImage(size: NSSize(width: 240, height: 150))
        image.lockFocus()
        NSColor.systemOrange.setFill()
        NSBezierPath(roundedRect: NSRect(x: 0, y: 0, width: 240, height: 150), xRadius: 12, yRadius: 12).fill()
        ("Synthetic image" as NSString).draw(at: NSPoint(x: 28, y: 65), withAttributes: [.font: NSFont.systemFont(ofSize: 22), .foregroundColor: NSColor.black])
        image.unlockFocus()
        let imagePath = output.appendingPathComponent("fixture.png")
        let imageData = NSBitmapImageRep(data: image.tiffRepresentation!)!.representation(using: .png, properties: [:])!
        try imageData.write(to: imagePath)
        let peers = [["device_id": "source", "device_name": "MacBook"],
                     ["device_id": "desktop", "device_name": "Mac Pro"],
                     ["device_id": "phone", "device_name": "iPhone"]]
        let status: [String: Any] = ["origin_device": "source", "uploaded_at": 1000,
                                    "supported": true, "peers": peers, "receivers": []]
        var confirmed = status
        confirmed["receivers"] = [["device_id": "desktop", "device_name": "Mac Pro", "received_at": 1100]]
        var complete = confirmed
        complete["receivers"] = [["device_id": "desktop", "device_name": "Mac Pro", "received_at": 1100],
                                 ["device_id": "phone", "device_name": "iPhone", "received_at": 1200]]
        let items = try [item(0, complete), item(1, confirmed), item(2, status), item(3, nil), item(4, confirmed, imagePath: imagePath.path)]
        for (name, appearance, scheme) in [("light", NSAppearance.Name.aqua, ColorScheme.light),
                                           ("dark", NSAppearance.Name.darkAqua, ColorScheme.dark)] {
            app.appearance = NSAppearance(named: appearance)
            let content = HStack(spacing: 12) {
                ForEach(Array(items.enumerated()), id: \.element.id) { index, item in
                    CardView(item: item, selected: index == 0, localDevice: "MacBook")
                }
            }
            .padding(24)
            .background(Color(nsColor: .windowBackgroundColor))
            .environment(\.colorScheme, scheme)
            let view = NSHostingView(rootView: content)
            view.frame = NSRect(x: 0, y: 0, width: 996, height: 228)
            view.layoutSubtreeIfNeeded()
            guard let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { fatalError("No native preview bitmap") }
            view.cacheDisplay(in: view.bounds, to: bitmap)
            guard let png = bitmap.representation(using: .png, properties: [:]) else { fatalError("No native PNG preview") }
            let file = output.appendingPathComponent("delivery-\(name).png")
            try png.write(to: file)
            print(file.path)
        }
    }
}

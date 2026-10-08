import XCTest
import SwiftUI
@testable import Pastazzo

final class WidgetAppearanceTests: XCTestCase {
    @MainActor
    func testWidgetAppearanceInLightAndDarkMode() throws {
        for scheme in [ColorScheme.light, .dark] {
            capture(QuickActionsContent(), name: "actions-\(scheme)", size: CGSize(width: 364, height: 170), scheme: scheme)
            for action in ClipboardAction.allCases {
                capture(CompactActionContent(action: action), name: "\(action.rawValue)-\(scheme)",
                    size: CGSize(width: 170, height: 170), scheme: scheme)
            }
        }
    }

    @MainActor
    private func capture<V: View>(_ content: V, name: String, size: CGSize, scheme: ColorScheme) {
        let view = content.padding(16).frame(width: size.width, height: size.height)
            .background(Color(uiColor: .systemBackground)).environment(\.colorScheme, scheme)
        let renderer = ImageRenderer(content: view)
        renderer.scale = 3
        let image = renderer.uiImage
        XCTAssertNotNil(image)
        if let image {
            let attachment = XCTAttachment(image: image)
            attachment.name = name
            attachment.lifetime = .keepAlways
            add(attachment)
        }
    }
}

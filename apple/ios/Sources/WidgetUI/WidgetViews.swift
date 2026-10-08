import SwiftUI
import WidgetKit
import UIKit

struct QuickActionsView: View {
    var body: some View {
        QuickActionsContent()
            .widgetURL(ClipboardAction.history.url)
            .pastazzoWidgetBackground()
    }
}

struct QuickActionsContent: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 8) {
                Image("PastazzoMark").resizable().scaledToFit().frame(width: 28, height: 28)
                Text("Pastazzo").font(.headline)
                Spacer()
            }
            HStack(spacing: 12) {
                Link(destination: ClipboardAction.paste.url) {
                    action("Paste", symbol: "clipboard", primary: true)
                }
                Link(destination: ClipboardAction.history.url) {
                    action("History", symbol: "clock.arrow.circlepath", primary: false)
                }
            }
        }
    }

    private func action(_ title: String, symbol: String, primary: Bool) -> some View {
        HStack(spacing: 8) {
            Image(systemName: symbol)
            Text(title).lineLimit(1).minimumScaleFactor(0.8)
        }
        .font(.headline)
        .frame(maxWidth: .infinity, minHeight: 54)
        .foregroundStyle(primary ? Color.white : Color.primary)
        .background(primary ? Color(red: 0.949, green: 0.4, blue: 0.106) : Color.primary.opacity(0.07),
                    in: RoundedRectangle(cornerRadius: 16))
    }
}

struct CompactActionView: View {
    let action: ClipboardAction
    var body: some View {
        CompactActionContent(action: action)
            .widgetURL(action.url)
            .pastazzoWidgetBackground()
    }
}

struct CompactActionContent: View {
    let action: ClipboardAction
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Image("PastazzoMark").resizable().scaledToFit().frame(width: 28, height: 28)
                Spacer()
            }
            Spacer(minLength: 0)
            Label(action == .paste ? "Paste" : "History",
                  systemImage: action == .paste ? "clipboard" : "clock.arrow.circlepath")
                .font(.title3.weight(.semibold))
            Text(action == .paste ? "Save your current copy" : "Open your copies")
                .font(.caption).foregroundStyle(.secondary).lineLimit(2)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
    }
}

private extension View {
    @ViewBuilder
    func pastazzoWidgetBackground() -> some View {
        if #available(iOS 17.0, *) {
            self.containerBackground(for: .widget) { Color(uiColor: .systemBackground) }
        } else {
            self.padding(16).background(Color(uiColor: .systemBackground))
        }
    }
}

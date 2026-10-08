import SwiftUI
import WidgetKit

private struct ActionsEntry: TimelineEntry {
    let date: Date
}

private struct ActionsProvider: TimelineProvider {
    func placeholder(in context: Context) -> ActionsEntry { ActionsEntry(date: Date()) }
    func getSnapshot(in context: Context, completion: @escaping (ActionsEntry) -> Void) {
        completion(ActionsEntry(date: Date()))
    }
    func getTimeline(in context: Context, completion: @escaping (Timeline<ActionsEntry>) -> Void) {
        completion(Timeline(entries: [ActionsEntry(date: Date())], policy: .never))
    }
}

private struct QuickActionsWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "PastazzoQuickActions", provider: ActionsProvider()) { _ in QuickActionsView() }
            .configurationDisplayName("Paste & History")
            .description("Save your current clipboard or open your Pastazzo history.")
            .supportedFamilies([.systemMedium])
    }
}

private struct PasteWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "PastazzoPaste", provider: ActionsProvider()) { _ in CompactActionView(action: .paste) }
            .configurationDisplayName("Paste")
            .description("Open Pastazzo and save your current clipboard.")
            .supportedFamilies([.systemSmall])
    }
}

private struct HistoryWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "PastazzoHistory", provider: ActionsProvider()) { _ in CompactActionView(action: .history) }
            .configurationDisplayName("History")
            .description("Open your Pastazzo clipboard history.")
            .supportedFamilies([.systemSmall])
    }
}

@main
struct PastazzoWidgets: WidgetBundle {
    var body: some Widget {
        QuickActionsWidget()
        PasteWidget()
        HistoryWidget()
    }
}

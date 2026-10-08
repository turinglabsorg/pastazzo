import SwiftUI

@main
struct PastazzoApp: App {
    @Environment(\.scenePhase) private var phase
    @StateObject private var startup = StartupModel()

    var body: some Scene {
        WindowGroup {
            Group {
                if let initializationError = startup.error {
                    VStack(spacing: 16) {
                        Image(systemName: "externaldrive.badge.exclamationmark").font(.largeTitle)
                        Text("Pastazzo couldn't open its storage").font(.headline)
                        Text(initializationError).foregroundStyle(.secondary)
                    }.padding()
                } else if let model = startup.history { ContentView(model: model) }
            }
            .task { if let model = startup.history { await model.load(); await model.importShared() } }
            .onChange(of: phase) { state in
                if state == .active, let model = startup.history { Task { await model.load(); await model.importShared() } }
            }
            .task(id: phase) {
                guard phase == .active, let model = startup.history else { return }
                while !Task.isCancelled {
                    try? await Task.sleep(nanoseconds: 5_000_000_000)
                    guard !Task.isCancelled else { return }
                    if model.connected { await model.load() }
                }
            }
        }
    }
}

@MainActor
private final class StartupModel: ObservableObject {
    let history: HistoryModel?
    let error: String?
    init() {
        do { history = HistoryModel(client: try MobileClient()); error = nil }
        catch { history = nil; self.error = error.localizedDescription }
    }
}

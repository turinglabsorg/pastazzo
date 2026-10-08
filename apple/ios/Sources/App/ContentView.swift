import SwiftUI
import UIKit

struct ContentView: View {
    @ObservedObject var model: HistoryModel
    @State private var query = ""
    @State private var filter = "All"
    @State private var settings = false
    @State private var scannerRequested = false
    @State private var pasteRequest: UUID?
    private let filters = ["All", "Text", "Images"]

    private var visibleItems: [MobileItem] {
        model.items.filter { item in
            (filter == "All" || item.kind == (filter == "Text" ? "text" : "image")) &&
            (query.isEmpty || item.preview.localizedCaseInsensitiveContains(query) || item.origin.localizedCaseInsensitiveContains(query))
        }
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    HStack(spacing: 8) {
                        Image(systemName: model.connected ? "lock.shield.fill" : "iphone")
                        Text(model.connected ? "Encrypted sync" : "On this iPhone")
                            .font(.subheadline.weight(.medium))
                        Spacer()
                        if model.busy { ProgressView() }
                    }
                    .foregroundStyle(.secondary)
                    HStack(spacing: 8) {
                        ForEach(filters, id: \.self) { name in
                            Button { filter = name } label: {
                                Text(name).font(.subheadline.weight(.semibold))
                                    .padding(.horizontal, 16).padding(.vertical, 8)
                                    .background(filter == name ? Color.orange.opacity(0.16) : Color(uiColor: .secondarySystemGroupedBackground), in: Capsule())
                            }
                            .buttonStyle(.plain)
                            .foregroundStyle(filter == name ? .orange : .secondary)
                        }
                        Spacer(minLength: 0)
                        PasteButton(payloadType: String.self) { values in
                            if let text = values.first { Task { await model.save(text: text) } }
                        }
                        .labelStyle(.titleAndIcon)
                        .disabled(model.busy)
                        .accessibilityLabel("Save text from clipboard")
                    }
                    if let message = model.message {
                        Text(message).font(.footnote).foregroundStyle(.secondary).accessibilityIdentifier("statusMessage")
                    }
                    if visibleItems.isEmpty {
                        VStack(spacing: 16) {
                            Image("PastazzoMark").resizable().frame(width: 88, height: 88)
                            Text(query.isEmpty ? "Keep a good copy." : "No matching copies")
                                .font(.title2.weight(.semibold))
                            Text(query.isEmpty ? "Save text with the paste button, or share text and images to Pastazzo from another app." : "Try a different search or filter.")
                                .font(.body).foregroundStyle(.secondary).multilineTextAlignment(.center)
                            if !model.connected && query.isEmpty {
                                Button("Connect your devices") { settings = true }.buttonStyle(.borderedProminent).tint(.orange)
                            }
                        }
                        .frame(maxWidth: .infinity).padding(.vertical, 52)
                    } else {
                        LazyVStack(spacing: 12) {
                            ForEach(visibleItems) { item in
                                ClipboardCard(item: item, client: model.client) { Task { await model.copy(item) } }
                            }
                        }
                    }
                }
                .padding(20)
            }
            .background(Color(uiColor: .systemGroupedBackground))
            .navigationTitle("Pastazzo")
            .searchable(text: $query, prompt: "Search copies or devices")
            .toolbar {
                ToolbarItem(placement: .navigationBarTrailing) {
                    Button { settings = true } label: { Image(systemName: "gearshape") }
                        .accessibilityLabel("Settings")
                }
            }
            .refreshable { await model.load() }
            .sheet(isPresented: $settings, onDismiss: { scannerRequested = false }) {
                SettingsView(model: model, scanOnAppear: scannerRequested)
            }
            .onOpenURL { url in
                if let action = ClipboardAction(url: url) {
                    settings = false
                    scannerRequested = false
                    query = ""
                    filter = "All"
                    pasteRequest = action == .paste ? UUID() : nil
                    return
                }
                guard url.scheme == "pastazzo", !model.connected else { return }
                if url.host == "scan" { scannerRequested = true; settings = true; return }
                guard url.host == "pair" else { return }
                settings = true
                Task { await model.pair(link: url.absoluteString) }
            }
            .task(id: pasteRequest) {
                if pasteRequest != nil { await model.pasteFromClipboard() }
            }
        }
        .tint(.orange)
    }
}

private struct ClipboardCard: View {
    let item: MobileItem
    let client: MobileClient
    let copy: () -> Void
    @State private var image: UIImage?

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Label(item.origin, systemImage: item.origin.lowercased().contains("mac") ? "desktopcomputer" : "iphone")
                    .lineLimit(1)
                Spacer()
                Text(item.date, style: .relative)
            }
            .font(.caption).foregroundStyle(.secondary)
            if item.kind == "image" {
                if let image {
                    Image(uiImage: image).resizable().scaledToFit().frame(maxHeight: 220)
                        .frame(maxWidth: .infinity).clipShape(RoundedRectangle(cornerRadius: 10))
                } else {
                    Label("Image", systemImage: "photo").frame(maxWidth: .infinity, minHeight: 100)
                        .foregroundStyle(.secondary)
                }
            } else {
                Text(item.preview).font(.body).lineLimit(7).frame(maxWidth: .infinity, alignment: .leading)
            }
            HStack {
                if item.queued { Label("Waiting to sync", systemImage: "clock").font(.caption).foregroundStyle(.secondary) }
                Spacer()
                Button(action: copy) { Label("Copy", systemImage: "doc.on.doc") }.font(.subheadline.weight(.semibold))
                    .accessibilityIdentifier("copy-\(item.id)")
            }
        }
        .padding(16)
        .background(Color(uiColor: .secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 16))
        .task(id: item.id) {
            guard item.kind == "image", let result = try? await client.call("item", ["id": item.id]),
                  let content = result["item"] as? [String: Any], let encoded = content["data"] as? String,
                  let data = HistoryModel.decodeBase64URL(encoded) else { return }
            image = UIImage(data: data)
        }
    }
}

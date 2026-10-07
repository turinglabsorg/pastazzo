import AppKit
import Carbon
import SwiftUI

/// What the shelf shows: the search and its results.
final class ShelfModel: ObservableObject {
    @Published var query = ""
    @Published private(set) var items: [HistoryItem] = []
    @Published var selection = 0

    private let store: HistoryStore
    private var pendingSearch: DispatchWorkItem?

    init(store: HistoryStore) {
        self.store = store
    }

    var selected: HistoryItem? { items.indices.contains(selection) ? items[selection] : nil }

    func search(delay: TimeInterval = 0.08) {
        pendingSearch?.cancel()
        let work = DispatchWorkItem { [weak self] in
            guard let self = self else { return }
            self.store.search(self.query) { items in
                self.items = items
                self.selection = 0
            }
        }
        pendingSearch = work
        DispatchQueue.main.asyncAfter(deadline: .now() + delay, execute: work)
    }

    func move(_ delta: Int) {
        guard !items.isEmpty else { return }
        selection = max(0, min(items.count - 1, selection + delta))
    }
}

/// A floating panel that takes the keyboard without activating the app, so
/// the app you were in stays frontmost and receives the paste.
final class ShelfPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}

final class ShelfController {
    private static let height: CGFloat = 268
    private static let margin: CGFloat = 12

    private let store: HistoryStore
    private let watcher: ClipboardWatcher
    private let sync: SyncMonitor
    private let model: ShelfModel
    private let panel: ShelfPanel
    private var monitors: [Any] = []
    /// When the shelf last closed: a click on the menu bar icon that closed
    /// it shouldn't open it again.
    private var closedAt = Date.distantPast

    init(store: HistoryStore, watcher: ClipboardWatcher, sync: SyncMonitor) {
        self.store = store
        self.watcher = watcher
        self.sync = sync
        model = ShelfModel(store: store)
        panel = ShelfPanel(
            contentRect: .zero,
            styleMask: [.nonactivatingPanel, .borderless, .fullSizeContentView],
            backing: .buffered,
            defer: true
        )
        panel.isFloatingPanel = true
        panel.level = .popUpMenu
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient]
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.hidesOnDeactivate = false
        NotificationCenter.default.addObserver(forName: NSWindow.didResignKeyNotification, object: panel, queue: .main) { [weak self] _ in
            self?.close()
        }
    }

    var isOpen: Bool { panel.isVisible }

    func toggle() {
        if isOpen {
            close()
        } else if Date().timeIntervalSince(closedAt) > 0.3 {
            open()
        }
    }

    func open() {
        guard !isOpen else { return }
        let mouse = NSEvent.mouseLocation
        guard let screen = NSScreen.screens.first(where: { NSMouseInRect(mouse, $0.frame, false) }) ?? NSScreen.main else { return }
        let visible = screen.visibleFrame
        panel.setFrame(
            NSRect(
                x: visible.minX + Self.margin,
                y: visible.minY + Self.margin,
                width: visible.width - 2 * Self.margin,
                height: Self.height
            ),
            display: false
        )
        model.query = ""
        model.search(delay: 0)
        // A fresh view each time, so the search field gets the focus again.
        panel.contentView = NSHostingView(rootView: ShelfView(
            model: model,
            sync: sync,
            onActivate: { [weak self] item, paste in self?.activate(item, paste: paste) },
            onClear: { [weak self] in self?.clearHistory() }
        ))
        panel.makeKeyAndOrderFront(nil)
        installMonitors()
    }

    func close() {
        guard isOpen else { return }
        monitors.forEach(NSEvent.removeMonitor)
        monitors.removeAll()
        panel.orderOut(nil)
        closedAt = Date()
    }

    private func installMonitors() {
        if let keys = NSEvent.addLocalMonitorForEvents(matching: .keyDown, handler: { [weak self] event in
            self?.handle(event) ?? event
        }) {
            monitors.append(keys)
        }
        if let clicks = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown], handler: { [weak self] _ in
            self?.close()
        }) {
            monitors.append(clicks)
        }
    }

    /// Arrows move, Return pastes, Escape closes; everything else types into
    /// the search field.
    private func handle(_ event: NSEvent) -> NSEvent? {
        switch Int(event.keyCode) {
        case kVK_Escape:
            close()
        case kVK_LeftArrow:
            model.move(-1)
        case kVK_RightArrow:
            model.move(1)
        case kVK_Return, kVK_ANSI_KeypadEnter:
            if let item = model.selected {
                activate(item, paste: true)
            }
        default:
            return event
        }
        return nil
    }

    /// Copies an item back to the clipboard, and pastes it if asked to.
    private func activate(_ item: HistoryItem, paste: Bool) {
        if item.isImage {
            guard let data = try? Data(contentsOf: URL(fileURLWithPath: item.path)),
                  PasteboardWriter.write(imageData: data)
            else { return }
        } else {
            PasteboardWriter.write(text: item.text)
        }
        watcher.ignoreCurrentContents()
        store.touch(item.id)
        NSSound(named: "Pop")?.play()
        close()
        if paste {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.12) { Paster.paste() }
        }
    }

    private func clearHistory() {
        close()
        let alert = NSAlert()
        alert.messageText = "Clear the clipboard history?"
        alert.informativeText = "Items already synced to your other devices stay there."
        alert.addButton(withTitle: "Clear")
        alert.addButton(withTitle: "Cancel")
        NSApp.activate(ignoringOtherApps: true)
        if alert.runModal() == .alertFirstButtonReturn {
            store.clear {}
        }
    }
}

struct ShelfView: View {
    @ObservedObject var model: ShelfModel
    @ObservedObject var sync: SyncMonitor
    let onActivate: (HistoryItem, Bool) -> Void
    let onClear: () -> Void
    @FocusState private var searchFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass").foregroundColor(.secondary)
                TextField("Search", text: $model.query)
                    .textFieldStyle(.plain)
                    .font(.system(size: 15))
                    .focused($searchFocused)
                if let transfer = sync.transferText {
                    Text(transfer)
                        .font(.system(size: 13).monospacedDigit())
                        .lineLimit(1)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 4)
                        .background(Capsule().fill(Color.accentColor.opacity(0.25)))
                }
                Button(action: onClear) { Image(systemName: "trash") }
                    .buttonStyle(.borderless)
                    .help("Clear history")
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(RoundedRectangle(cornerRadius: 8).fill(Color.primary.opacity(0.07)))

            if model.items.isEmpty {
                Text(model.query.isEmpty ? "Copy something to start your history." : "No matches.")
                    .foregroundColor(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollViewReader { proxy in
                    ScrollView(.horizontal, showsIndicators: false) {
                        LazyHStack(spacing: 8) {
                            ForEach(Array(model.items.enumerated()), id: \.element.id) { index, item in
                                CardView(item: item, selected: index == model.selection, localDevice: sync.localDevice)
                                    .id(item.id)
                                    // Like the GNOME shelf: one click copies, two paste.
                                    .gesture(
                                        TapGesture(count: 2).onEnded { onActivate(item, true) }
                                            .exclusively(before: TapGesture().onEnded { onActivate(item, false) })
                                    )
                            }
                        }
                    }
                    .onChange(of: model.selection) { index in
                        guard model.items.indices.contains(index) else { return }
                        withAnimation(.easeOut(duration: 0.15)) { proxy.scrollTo(model.items[index].id) }
                    }
                }
            }
        }
        .padding(14)
        .background(VisualEffectView().clipShape(RoundedRectangle(cornerRadius: 14)))
        .onChange(of: model.query) { _ in model.search() }
        .onAppear { DispatchQueue.main.async { searchFocused = true } }
    }
}

struct CardView: View {
    let item: HistoryItem
    let selected: Bool
    /// This Mac's name once it syncs, shown on copies made here.
    let localDevice: String

    private var origin: String { item.origin.isEmpty ? localDevice : item.origin }

    private static let relative: RelativeDateTimeFormatter = {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .short
        return formatter
    }()

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 4) {
                Image(systemName: item.isImage ? "photo" : "text.alignleft")
                Text(Self.relative.localizedString(for: item.date, relativeTo: Date()))
                Spacer()
                if !origin.isEmpty {
                    Text(origin).lineLimit(1).truncationMode(.tail)
                }
            }
            .font(.caption)
            .foregroundColor(.secondary)

            if item.isImage, let image = Thumbnails.shared.image(at: item.path) {
                Image(nsImage: image)
                    .resizable()
                    .scaledToFit()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                Text(item.preview)
                    .font(.system(size: 12))
                    .lineLimit(9)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            }
        }
        .padding(10)
        .frame(width: 180, height: 180)
        .background(RoundedRectangle(cornerRadius: 10).fill(Color(nsColor: .textBackgroundColor).opacity(0.9)))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(selected ? Color.accentColor : Color.clear, lineWidth: 3))
        .contentShape(Rectangle())
    }
}

/// Image previews, decoded once.
final class Thumbnails {
    static let shared = Thumbnails()
    private let cache = NSCache<NSString, NSImage>()

    func image(at path: String) -> NSImage? {
        if let image = cache.object(forKey: path as NSString) {
            return image
        }
        guard let image = NSImage(contentsOfFile: path) else { return nil }
        cache.setObject(image, forKey: path as NSString)
        return image
    }
}

struct VisualEffectView: NSViewRepresentable {
    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = .popover
        view.blendingMode = .behindWindow
        view.state = .active
        return view
    }

    func updateNSView(_ view: NSVisualEffectView, context: Context) {}
}

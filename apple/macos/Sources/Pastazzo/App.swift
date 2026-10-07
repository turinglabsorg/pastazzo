import AppKit
import Carbon
import Combine

final class AppDelegate: NSObject, NSApplicationDelegate {
    private let store = HistoryStore()
    private var statusItem: NSStatusItem?
    private var watcher: ClipboardWatcher?
    private var inbox: InboxWatcher?
    private var shelf: ShelfController?
    private var hotKey: HotKey?
    private var sync: SyncMonitor?
    private var settings: SettingsWindowController?
    private var menu: NSMenu?
    private var subscriptions: Set<AnyCancellable> = []

    func applicationDidFinishLaunching(_ notification: Notification) {
        guard store.isAvailable else {
            let alert = NSAlert()
            alert.messageText = "The pastazzo CLI is missing"
            alert.informativeText = "Pastazzo keeps its history with \(store.cli.path). Install it with apple/macos/install.sh from the pastazzo repository."
            NSApp.activate(ignoringOtherApps: true)
            alert.runModal()
            NSApp.terminate(nil)
            return
        }

        let watcher = ClipboardWatcher(store: store)
        let inbox = InboxWatcher(directory: store.inboxDirectory, watcher: watcher)
        let sync = SyncMonitor(dataDirectory: store.dataDirectory)
        let shelf = ShelfController(store: store, watcher: watcher, sync: sync)
        watcher.start()
        inbox.start()
        sync.start()
        self.sync = sync
        settings = SettingsWindowController(store: store)
        // ⇧⌥V, like Shift+Alt+V on GNOME.
        hotKey = HotKey(keyCode: kVK_ANSI_V, modifiers: shiftKey | optionKey) { [weak shelf] in shelf?.toggle() }
        self.watcher = watcher
        self.inbox = inbox
        self.shelf = shelf
        statusItem = makeStatusItem()
        // Transfers, and devices waiting for approval, show next to the icon.
        sync.$transferShort
            .combineLatest(sync.$waitingDevices)
            .receive(on: RunLoop.main)
            .sink { [weak self] transfer, waiting in
                let parts = [waiting > 0 ? "New device" : nil, transfer].compactMap { $0 }
                self?.statusItem?.button?.title = parts.isEmpty ? "" : " " + parts.joined(separator: "  ")
            }
            .store(in: &subscriptions)
    }

    /// A click opens the shelf; a right-click (or Control-click) shows the menu.
    private func makeStatusItem() -> NSStatusItem {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        item.button?.image = NSImage(systemSymbolName: "doc.on.clipboard", accessibilityDescription: "Pastazzo")
        item.button?.imagePosition = .imageLeft
        item.button?.target = self
        item.button?.action = #selector(statusItemClicked)
        item.button?.sendAction(on: [.leftMouseUp, .rightMouseUp])

        let menu = NSMenu()
        menu.addItem(withTitle: "Open Pastazzo   ⇧⌥V", action: #selector(openShelf), keyEquivalent: "").target = self
        menu.addItem(withTitle: "Settings…", action: #selector(openSettings), keyEquivalent: ",").target = self
        menu.addItem(.separator())
        menu.addItem(withTitle: "Quit Pastazzo", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        self.menu = menu
        return item
    }

    @objc private func statusItemClicked() {
        let event = NSApp.currentEvent
        if event?.type == .rightMouseUp || event?.modifierFlags.contains(.control) == true {
            // Shown only for this click, so a plain click keeps opening the shelf.
            statusItem?.menu = menu
            statusItem?.button?.performClick(nil)
            statusItem?.menu = nil
        } else if sync?.waitingDevices ?? 0 > 0 {
            // Someone is waiting: that's what the click is for.
            settings?.show()
        } else {
            shelf?.toggle()
        }
    }

    @objc private func openShelf() {
        shelf?.open()
    }

    @objc private func openSettings() {
        settings?.show()
    }
}

import AppKit
import Carbon

final class AppDelegate: NSObject, NSApplicationDelegate {
    private let store = HistoryStore()
    private var statusItem: NSStatusItem?
    private var watcher: ClipboardWatcher?
    private var inbox: InboxWatcher?
    private var shelf: ShelfController?
    private var hotKey: HotKey?

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
        let shelf = ShelfController(store: store, watcher: watcher)
        watcher.start()
        inbox.start()
        // ⇧⌥V, like Shift+Alt+V on GNOME.
        hotKey = HotKey(keyCode: kVK_ANSI_V, modifiers: shiftKey | optionKey) { [weak shelf] in shelf?.toggle() }
        self.watcher = watcher
        self.inbox = inbox
        self.shelf = shelf
        statusItem = makeStatusItem()
    }

    private func makeStatusItem() -> NSStatusItem {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.image = NSImage(systemSymbolName: "doc.on.clipboard", accessibilityDescription: "Pastazzo")
        let menu = NSMenu()
        menu.addItem(withTitle: "Open Pastazzo   ⇧⌥V", action: #selector(openShelf), keyEquivalent: "").target = self
        menu.addItem(withTitle: "Allow Pasting…", action: #selector(allowPasting), keyEquivalent: "").target = self
        menu.addItem(.separator())
        menu.addItem(withTitle: "Quit Pastazzo", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        item.menu = menu
        return item
    }

    @objc private func openShelf() {
        shelf?.open()
    }

    /// Double-click and Return paste by sending ⌘V, which macOS only allows
    /// once Pastazzo is enabled under Privacy & Security → Accessibility.
    @objc private func allowPasting() {
        if Paster.isTrusted {
            let alert = NSAlert()
            alert.messageText = "Pasting is allowed"
            alert.informativeText = "Double-click an item, or select it and press Return, to paste it into the app you were using."
            NSApp.activate(ignoringOtherApps: true)
            alert.runModal()
        } else {
            Paster.requestAccess()
        }
    }
}

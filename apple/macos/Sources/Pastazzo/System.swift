import AppKit
import ApplicationServices
import Carbon

/// A global keyboard shortcut. Carbon hot keys work without any permission.
final class HotKey {
    private var hotKey: EventHotKeyRef?
    private var handler: EventHandlerRef?
    private let action: () -> Void

    init(keyCode: Int, modifiers: Int, action: @escaping () -> Void) {
        self.action = action
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let status = InstallEventHandler(
            GetApplicationEventTarget(),
            { _, _, userData in
                guard let userData = userData else { return noErr }
                Unmanaged<HotKey>.fromOpaque(userData).takeUnretainedValue().action()
                return noErr
            },
            1,
            &spec,
            Unmanaged.passUnretained(self).toOpaque(),
            &handler
        )
        let id = EventHotKeyID(signature: OSType(0x5053_545A), id: 1) // "PSTZ"
        if status != noErr
            || RegisterEventHotKey(UInt32(keyCode), UInt32(modifiers), id, GetApplicationEventTarget(), 0, &hotKey) != noErr {
            NSLog("Pastazzo: couldn't register the keyboard shortcut")
        }
    }

    deinit {
        if let hotKey = hotKey {
            UnregisterEventHotKey(hotKey)
        }
        if let handler = handler {
            RemoveEventHandler(handler)
        }
    }
}

/// Pastes into the frontmost app by sending it ⌘V, which needs the
/// Accessibility permission.
enum Paster {
    static var isTrusted: Bool { AXIsProcessTrusted() }

    /// Shows the system prompt that leads to System Settings.
    static func requestAccess() {
        let prompt = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
        _ = AXIsProcessTrustedWithOptions([prompt: true] as CFDictionary)
    }

    static func paste() {
        guard isTrusted else {
            requestAccess()
            return
        }
        let source = CGEventSource(stateID: .combinedSessionState)
        for keyDown in [true, false] {
            let event = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(kVK_ANSI_V), keyDown: keyDown)
            event?.flags = .maskCommand
            event?.post(tap: .cghidEventTap)
        }
    }
}

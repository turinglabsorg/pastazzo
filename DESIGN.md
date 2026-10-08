# Pastazzo design

Pastazzo is a quiet clipboard shelf with a citrus identity. Content and its provenance come first.

## Apple interfaces

- Use system backgrounds and semantic label colors, with native light and dark appearance.
- Citrus accent: orange `#F2661B`; lighter icon highlights `#FFC15E`. Use the accent for primary actions and selected filters.
- Typography: SF Pro, system dynamic type. Large titles for the iOS shelf; headline for section titles; body for clipboard previews; caption for origin, time, and sync status. Monospaced text is reserved for approval codes and fingerprints.
- Spacing follows 4, 8, 12, 16, 20, and 24 point steps. iOS content has 20 point horizontal margins. macOS settings use 20 point margins.
- Cards use 16 point radii on iOS, 8 on macOS. Fields, sheets, confirmations, and navigation use native controls.
- Every clipboard card shows its device. Local items use the computer name even before login. Additional source metadata must distinguish the application observed at copy time from a verified sending device.
- Sync state must distinguish local use, connecting, waiting for approval, connected, and a connection error. Never hide a failed operation behind a connected label.
- iOS reads the clipboard only after a deliberate paste action. Receiving history never writes the system clipboard; tapping Copy does.
- iOS setup starts with Scan Mac QR. The Mac shows a high-contrast temporary QR on white with a quiet zone, expiry, and a confirmation for the requesting device. Manual server, fingerprint, username, and secure password fields live under Connect manually. Passwords never go in command arguments or logs.
- Empty shelves explain how to add the first item. Tests and screenshots use explicitly seeded demo content only.
- Home Screen widgets use the original citrus mark and semantic system backgrounds. The medium widget has equally accessible Paste and History actions; small widgets offer one action each. Paste opens the app and imports the current clipboard only on that explicit request. Widgets show no private clipboard previews and hold no account keys.

## Android interfaces

- Use Kotlin and Material 3 with system sans-serif typography, semantic light/dark colors, the existing citrus accent, and the original Pastazzo icon. Keep 20 dp content margins, 16 dp history cards, and native controls that scale with the system font setting.
- Pair bright citrus action backgrounds with dark ink. Use deeper orange `#B4440D` for light-mode text accents and `#FFAD73` for dark-mode accents; shelf surfaces use warm neutrals rather than the default Material purple.
- The shelf puts device origin above each text or image entry, followed by its date, queued state, and explicit Copy action. Search and All/Text/Images filters remain visible above the history. Empty history explains the Paste action.
- Settings starts with Scan Mac QR and shows the approval code while waiting for the Mac. A scanned or externally opened pairing link requires Connect confirmation and displays the server host without exposing the temporary capability.
- Paste reads the clipboard only while the activity has focus after a deliberate button or widget action. Receiving and opening History do not alter or import the clipboard. Public Paste links require confirmation; the immutable widget PendingIntent uses a private activity alias.
- The Home Screen widget has the citrus mark and equal Paste and History actions, light/dark backgrounds, and no clipboard previews or account keys. Share targets import only the text or image explicitly provided by the sending app.

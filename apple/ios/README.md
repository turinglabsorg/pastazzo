# Pastazzo for iOS

The native SwiftUI app uses `pastazzo-mobile`, a Rust static library built on the same OPAQUE, device approval, encrypted item, and signed request implementation as the desktop client. It supports text, images, device origins, search, local history, an encrypted offline outbox, and a share extension.

## Build

Requirements: Xcode, Rust, XcodeGen, and the `aarch64-apple-ios` and `aarch64-apple-ios-sim` Rust targets. The app supports iOS 16 or later. The simulator library targets Apple Silicon.

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
sh apple/ios/build-rust.sh
cd apple/ios
xcodegen generate
open Pastazzo.xcodeproj
```

For a physical device, select your Apple development team for the app and both extension targets and register the App Group `group.org.pastazzo.clipboard`. If your team requires different bundle identifiers, update both targets and the App Group in `project.yml` and `SharedInbox.swift`, then regenerate the project. App Store distribution and push notifications are not configured.

## Connect

On a connected Mac, open Settings → Show Pairing QR. On iPhone, open Settings → Scan Mac QR, scan the code, then confirm the requesting iPhone on the Mac. The QR expires after five minutes and is bound to the first requesting device. It contains a temporary pairing capability and public trust information, never a password or account keys. The Mac encrypts the approval to the iPhone's own keys and signs it with the public key carried in the QR. The account fingerprints must then match.

Connect manually remains available for a server URL, username, password, and pinned server fingerprint. Approve that login only after comparing the codes on both devices.

The app refreshes while active. iOS does not provide a general background clipboard monitor; this app does not promise continuous background sync. Received items enter history without changing the system clipboard. Tap Copy to write a local-only pasteboard item, or use the native paste button to deliberately import text or an image. Share text, URLs, or an image from another app to save it in the protected shared inbox; open Pastazzo to import and sync it.

Paste and Share use the same item-provider reader, including file-backed JPEG and HEIC photos. Images take priority over accompanying captions or URLs. ImageIO applies their orientation and converts the first frame to PNG, with a maximum long edge of 3072 pixels (2048 if needed to fit the 25 MB image limit). This bounds extension memory and produces a portable image for desktop clients. Invalid images report an error instead of silently saving their caption.

The physical build must embed `PastazzoShare.appex`; both the app and share extension provisioning profiles must include `group.org.pastazzo.clipboard`. In Xcode, select the development team in Signing & Capabilities for both targets and let provisioning finish before building. A build without the share extension or its App Group cannot receive photos from the system share sheet. The widget target needs no App Group.

Device keys use the protected local keychain, accessible only while unlocked and never synchronized through iCloud. Clipboard files use private permissions and protected application storage excluded from backups. The share extension does not hold account keys or make network requests. Failed uploads retain their original ciphertext and item id for an idempotent retry. Clearing local history cancels queued uploads on that iPhone.

History groups identical text and image payloads into one card, using the latest copy's date and device. Comparison uses complete content, not the truncated preview. Existing duplicate files are grouped automatically; any pending upload remains visible as queued and retains its original encrypted item id.

While refreshing, the app sends signed, encrypted device receipts after saving incoming items. The Mac shelf can then show this iPhone as a confirmed receiver. Receipts have their own durable outbox and optional server feed; older servers keep syncing normally while confirmations remain queued.

## Home Screen widgets

Add Pastazzo from the iOS widget gallery. Paste & History is a medium widget with both actions; Paste and History are separate small widgets. Paste opens the app and imports the current text or image, then uses the ordinary encrypted outbox and sync flow. The iOS paste permission prompt applies. History opens the complete shelf and resets search and filters. These actions also use `pastazzo://paste` and `pastazzo://history`.

The widget extension holds no account keys, reads no clipboard or history files, and makes no network requests. It needs no App Group. It is available independently of the share extension's App Group provisioning.

## Verify

```sh
cargo test --workspace
cd apple/ios
xcodebuild test -project Pastazzo.xcodeproj -scheme Pastazzo \
  -destination 'platform=iOS Simulator,name=iPhone 16 Pro' \
  -derivedDataPath build CODE_SIGNING_ALLOWED=YES CODE_SIGN_IDENTITY=-
```

Rust integration tests use a real HTTP server and approved desktop/mobile devices. Native tests cover persistence through the C bridge, path rejection, explicit clipboard writes, file-backed photo share import across a restart without duplicate items, JPEG/HEIC conversion and orientation, image precedence over captions, invalid images, widget action URL validation, foreground text/image paste, and widget rendering in light and dark appearance. Simulator builds use ad-hoc signing so the share extension receives its App Group entitlement; disabling signing cannot test the real shared container. Verify Photos → Share → Pastazzo → Post, then reopen Pastazzo and check the image card. Physical-device signing still requires device testing.

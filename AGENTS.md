# Pastazzo

Clipboard history for GNOME on Wayland and macOS, with optional end-to-end encrypted sync. The README explains the pieces; `docs/SECURITY.md` and `docs/PROTOCOL.md` are the threat model and the wire format.

## Build and test

```bash
cargo build --release
cargo test --workspace
cd test-harness && npm install && npm test   # GNOME shelf UI, mocked backend
```

The macOS app builds only on a Mac, from Terminal on the Mac itself: `sh apple/macos/install.sh`.

The iOS app lives in `apple/ios`; read its README for Rust framework generation, XcodeGen, signing, and native simulator tests. Keep cryptography and sync in `pastazzo-mobile` using the shared Rust client. The share extension only writes the protected App Group inbox; it never sends plaintext to the network.

The Android app lives in `android`; read its README for the NDK Rust build, Gradle, and emulator instrumentation. It uses Kotlin/Material 3 and the same Rust engine through JNI. Run the isolated `android_fixture` example and ADB loopback port forwarding before its integration tests.

## Architecture and entry points

- `pastazzo-mobile` exposes a JSON C bridge (`pastazzo_mobile_call` / `pastazzo_mobile_free`) to SwiftUI. It manages text/image history, device origins, local search data, and encrypted uploads. iOS account and device keys require the protected local keychain; there is no file-key fallback.
- The share extension stages items in `group.org.pastazzo.clipboard`; the app imports them using stable item ids to prevent duplicates. The widget extension has no clipboard, history, keychain, or network access. Its `pastazzo://paste` and `pastazzo://history` links open the app for explicit paste or history actions. Sync runs while the iOS app is active.
- QR pairing lives in `pastazzo-core::pairing`, `pastazzo-sync::pairing`, and the server's `app::pairing`. A trusted Mac displays a five-minute capability and confirms the requesting device. The recipient verifies the owner's signature and the pairing/account/device bindings before accepting the encrypted key grant.
- Pairing endpoints are `POST /v1/pairings`, `GET` / `DELETE /v1/pairings/{id}`, `POST /v1/pairings/{id}/request`, and `PUT /v1/pairings/{id}/grant`. Sessions are held in memory, bind the first valid peer, and expire on timeout, cancellation, owner revocation, or server restart. Shared JSON schemas live in `pastazzo-core::api`; the full format is in `docs/PROTOCOL.md`.
- Desktop and mobile outboxes persist sealed items and reuse their original ciphertext and item ids after failures or restarts. Identical uploads return the original server cursor; conflicting reuse of an item id returns HTTP 409.
- Mobile history groups identical full text or image content using the shared local fingerprint, retaining the newest entry's date and device and aggregating pending upload status. Stored events and their sealed outbox ids remain intact; pruning and clearing operate on every stored entry, including hidden duplicates.
- `pastazzo-sync backup` pipes the existing private recovery state directly into hush and returns only an encrypted envelope. Recovery and credential replacement are documented in `docs/RECOVERY.md`.
- Android supplies `SecretBackend` through the JNI bridge in `pastazzo-mobile::android`. A nonexportable, unlocked-device-only Android Keystore AES-GCM key wraps each account/device secret bundle in private storage excluded from backup; there is no plaintext fallback. Connecting requires a secure screen lock. Account keys still reach Rust only for local cryptographic operations.
- Android clipboard access lives in `ClipboardAccess`; reads require an explicit focused activity action. The widget's immutable PendingIntent targets the private `PasteShortcut` alias. Public Paste links require confirmation, History never imports the clipboard, and the share target uses only its provided payload. Production QR setup accepts HTTPS; loopback HTTP is debug-only.
- Android's package is `org.pastazzo.android`, with Android 10 as its minimum version and ARM64 as the current packaged ABI. `android/build-rust.sh` generates the 16 KB-aligned JNI library; Gradle builds development APKs. Release signing is not configured.
- Android instrumentation substitutes `TestApplication` with a separate temporary storage root and uses only synthetic accounts on the loopback fixture. `android/test-emulator.sh` refuses physical-device targets. Capture light/dark screenshots outside the instrumentation lifecycle.

## Rules

- The history is the user's clipboard: passwords, tokens, private screenshots. The `pastazzo` CLI is the only writer of `~/.local/share/pastazzo/items`; the GNOME extension and the macOS app call it. Its directories are 0700 and its files 0600: create them through `history_dir()` and `write_private()`, never with `fs::write` or `File::create`.
- `pastazzo-sync` writes received items to the inbox with the same modes (`make_inbox()`, files 0600). Keys live in the system keychain, else in `~/.config/pastazzo/sync.json` at 0600.
- The server never sees plaintext. A change to the protocol updates `docs/PROTOCOL.md`, the test vectors, and `docs/SECURITY.md` together.
- After any install or update on GNOME, log out and back in before testing the extension.

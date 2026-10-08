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

## Rules

- The history is the user's clipboard: passwords, tokens, private screenshots. The `pastazzo` CLI is the only writer of `~/.local/share/pastazzo/items`; the GNOME extension and the macOS app call it. Its directories are 0700 and its files 0600: create them through `history_dir()` and `write_private()`, never with `fs::write` or `File::create`.
- `pastazzo-sync` writes received items to the inbox with the same modes (`make_inbox()`, files 0600). Keys live in the system keychain, else in `~/.config/pastazzo/sync.json` at 0600.
- The server never sees plaintext. A change to the protocol updates `docs/PROTOCOL.md`, the test vectors, and `docs/SECURITY.md` together.
- After any install or update on GNOME, log out and back in before testing the extension.

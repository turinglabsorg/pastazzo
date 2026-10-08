# Pastazzo for Android

The native Kotlin/Material 3 app uses the same `pastazzo-mobile` Rust engine as iOS through JNI. It supports QR device approval, text and images, device origins, searchable local history, encrypted offline retries, explicit Copy/Paste, Android sharing, and a Paste & History Home Screen widget.

## Build

Requirements: JDK 17, Rust, Android SDK platform/build-tools 35, and NDK 27.2.12479018. The app supports Android 10 or later. This build packages ARM64, including the Apple Silicon Android emulator. The Gradle wrapper is pinned and verifies its distribution checksum. Generated Rust libraries and build products are excluded from Git.

```sh
export ANDROID_HOME=/path/to/Android/sdk
"$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" \
  'platforms;android-35' 'build-tools;35.0.0' 'ndk;27.2.12479018'
rustup target add aarch64-linux-android
sh android/build-rust.sh
cd android
./gradlew :app:assembleDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

`ANDROID_NDK_HOME` can select another compatible NDK. The Rust shared library uses 16 KB ELF segment alignment. Release signing and Google Play distribution are not configured; the development APK uses the standard local Android debug identity.

## Use

Set a secure screen lock on Android before connecting. On a connected Mac, open Pastazzo Settings → Show Pairing QR. On Android, open Settings → Scan Mac QR. Confirm Connect on the phone, compare the approval code, and approve that phone on the Mac. Pairing uses the existing signed, recipient-encrypted grant and server fingerprint pinning; no account password is needed. Production pairing requires an HTTPS server. Debug builds additionally permit loopback HTTP for isolated tests.

Tap Paste to import the current text or image and send it through the encrypted outbox. Tap Copy on a history card to place that item on the Android clipboard. Received items enter history without changing the clipboard. Share text or an image to Pastazzo through Android's share sheet. Add the Pastazzo widget for Paste and History shortcuts; public Paste links require an additional confirmation to prevent unsolicited clipboard reads.

The app refreshes while active and retries pending uploads when it reconnects. It does not continuously read other apps' clipboards or promise continuous background sync. Text is limited to 1 MB and images to 25 MB. Clearing local history also cancels this phone's queued uploads. Disconnect revokes its device identity and removes its protected account/device keys.

## Key storage

Android Keystore holds a nonexportable AES-GCM wrapping key, restricted to use while the device is unlocked. Account/device secret bundles are authenticated and encrypted with that key in the app's `noBackupFilesDir`; there is no plaintext fallback. The Rust state file contains public connection metadata and a keychain reference. Private directories are 0700 and files 0600. Clipboard history stays in private app storage, excluded from backup and device transfer. Explicit image copies use a bounded private cache exposed only through temporary clipboard URI grants. Clipboard previews are marked sensitive.

## Verify

Start an ARM64 Android emulator with a secure screen lock configured and leave it unlocked. Run the isolated Rust fixture server in a separate terminal; it creates an in-memory server/account and automatically approves only its synthetic test pairing. It does not use any production credentials or server state. Both fixture listeners bind to loopback.

```sh
cargo run -p pastazzo-mobile --example android_fixture
```

Then:

```sh
adb reverse tcp:32951 tcp:32951
adb reverse tcp:32952 tcp:32952
cd android
./gradlew :app:connectedDebugAndroidTest :app:lintDebug
```

Alternatively, `ANDROID_SERIAL=emulator-5554 sh android/test-emulator.sh` builds, installs, and runs the same suite while retaining the installed app. This script refuses physical-device targets. The custom instrumentation application uses a separate temporary storage root, so tests never connect through an existing app account.

Instrumentation verifies Keystore authentication and nonexportability, private file modes, Rust/JNI text and image persistence, traversal rejection, QR decoding and approval, on-screen connection and disconnection, encrypted interoperability with desktop, durable retry across a server outage, explicit text/image Paste/Copy, share imports, real widget PendingIntents, public link confirmation, and actual camera-scanner launch. These tests use synthetic clipboard content only. Check light/dark rendering on the emulator with Android's system appearance settings and `adb exec-out screencap -p`, outside the instrumentation lifecycle. Stop the fixture server after the tests.

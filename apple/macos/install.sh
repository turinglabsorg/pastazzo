#!/bin/sh
# Builds and installs pastazzo on macOS with the Command Line Tools and Cargo
# (no Xcode needed): the pastazzo and pastazzo-sync CLIs in ~/.local/bin,
# Pastazzo.app in /Applications, and launch agents that start the app and
# sync at login. Run it again to update.
set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
MACOS="$ROOT/apple/macos"
BIN="$HOME/.local/bin"
AGENTS="$HOME/Library/LaunchAgents"
APP=/Applications/Pastazzo.app
DOMAIN="gui/$(id -u)"

echo "building the pastazzo CLIs"
cargo build --release --manifest-path "$ROOT/Cargo.toml" -p pastazzo -p pastazzo-sync
mkdir -p "$BIN"
install -m 0755 "$ROOT/target/release/pastazzo" "$ROOT/target/release/pastazzo-sync" "$BIN/"

echo "building Pastazzo.app"
BUILD="$MACOS/build/Pastazzo.app"
rm -rf "$BUILD"
mkdir -p "$BUILD/Contents/MacOS"
# swiftc directly: SwiftPM can't load manifests with some Command Line Tools.
swiftc -O -target "$(uname -m)-apple-macos12.0" -sdk "$(xcrun --show-sdk-path)" -framework Carbon \
    -o "$BUILD/Contents/MacOS/Pastazzo" "$MACOS"/Sources/Pastazzo/*.swift
cp "$MACOS/Info.plist" "$BUILD/Contents/Info.plist"
codesign --force --sign - "$BUILD"

# Stop what's running before replacing it.
launchctl bootout "$DOMAIN/org.pastazzo.app" 2>/dev/null || true
rm -rf "$APP"
cp -R "$BUILD" "$APP"

mkdir -p "$AGENTS"
cat > "$AGENTS/org.pastazzo.app.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>org.pastazzo.app</string>
	<key>ProgramArguments</key>
	<array>
		<string>$APP/Contents/MacOS/Pastazzo</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>LimitLoadToSessionType</key>
	<string>Aqua</string>
	<key>ProcessType</key>
	<string>Interactive</string>
</dict>
</plist>
EOF
launchctl bootstrap "$DOMAIN" "$AGENTS/org.pastazzo.app.plist"

# Sync runs whenever this Mac is logged in to a pastazzo account. (Re)loading
# it also makes it notice the app, and switch to the app's history.
sed "s#/Users/YOU#$HOME#g" "$ROOT/contrib/launchd/org.pastazzo.sync.plist" > "$AGENTS/org.pastazzo.sync.plist"
launchctl bootout "$DOMAIN/org.pastazzo.sync" 2>/dev/null || true
launchctl bootstrap "$DOMAIN" "$AGENTS/org.pastazzo.sync.plist"

echo "installed $APP"
echo "to paste with a double-click or Return, allow Pastazzo in System Settings → Privacy & Security → Accessibility"

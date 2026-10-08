#!/bin/sh
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
IOS="$ROOT/apple/ios"
export IPHONEOS_DEPLOYMENT_TARGET=16.0
cargo build --manifest-path "$ROOT/Cargo.toml" --release -p pastazzo-mobile --target aarch64-apple-ios
cargo build --manifest-path "$ROOT/Cargo.toml" --release -p pastazzo-mobile --target aarch64-apple-ios-sim
mkdir -p "$IOS/Frameworks"
OUTPUT="$IOS/Frameworks/PastazzoMobile.xcframework"
if [ -d "$OUTPUT" ]; then rm -rf "$OUTPUT"; fi
xcodebuild -create-xcframework \
    -library "$ROOT/target/aarch64-apple-ios/release/libpastazzo_mobile.a" -headers "$IOS/Include" \
    -library "$ROOT/target/aarch64-apple-ios-sim/release/libpastazzo_mobile.a" -headers "$IOS/Include" \
    -output "$OUTPUT"

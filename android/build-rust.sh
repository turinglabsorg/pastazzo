#!/bin/sh
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
SDK=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-"$HOME/Library/Android/sdk"}}
NDK=${ANDROID_NDK_HOME:-"$SDK/ndk/27.2.12479018"}
case $(uname -s) in Darwin) HOST=darwin-x86_64 ;; Linux) HOST=linux-x86_64 ;; *) echo 'Use macOS or Linux to build Android.' >&2; exit 1 ;; esac
TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/$HOST/bin"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$TOOLCHAIN/aarch64-linux-android29-clang"
export CC_aarch64_linux_android="$CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER"
export AR_aarch64_linux_android="$TOOLCHAIN/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS='-C link-arg=-Wl,-z,max-page-size=16384'
cargo build --manifest-path "$ROOT/Cargo.toml" --locked --release -p pastazzo-mobile --target aarch64-linux-android
DEST="$ROOT/android/app/src/main/jniLibs/arm64-v8a"
mkdir -p "$DEST"
cp "$ROOT/target/aarch64-linux-android/release/libpastazzo_mobile.so" "$DEST/"

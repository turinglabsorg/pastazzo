#!/usr/bin/env bash
set -euo pipefail

REPO_URL="${PASTAZZO_REPO_URL:-https://github.com/turinglabsorg/pastazzo.git}"
REF="${PASTAZZO_REF:-main}"
SRC_DIR="${PASTAZZO_SRC_DIR:-$HOME/.local/src/pastazzo}"
BIN_DIR="$HOME/.local/bin"
BIN_PATH="$BIN_DIR/pastazzo"
EXT_UUID="pastazzo@turinglabs.org"
OLD_EXT_UUID="pastebar@turinglabs"
EXT_DIR="$HOME/.local/share/gnome-shell/extensions/$EXT_UUID"
OLD_DATA_DIR="$HOME/.local/share/pastebar/items"
DATA_DIR="$HOME/.local/share/pastazzo/items"

need_cmd() {
  command -v "$1" >/dev/null 2>&1
}

install_missing_deps() {
  local missing=()

  need_cmd git || missing+=(git)
  need_cmd cargo || missing+=(cargo)
  need_cmd glib-compile-schemas || missing+=(libglib2.0-bin)
  need_cmd gnome-extensions || missing+=(gnome-shell-common)

  if [ "${#missing[@]}" -eq 0 ]; then
    return
  fi

  if need_cmd apt-get && need_cmd sudo; then
    echo "Installing missing packages: ${missing[*]}"
    sudo apt-get update
    sudo apt-get install -y "${missing[@]}"
    return
  fi

  echo "Missing required commands. Install git, cargo, glib-compile-schemas and gnome-extensions, then rerun this script." >&2
  exit 1
}

sync_source() {
  mkdir -p "$(dirname "$SRC_DIR")"

  if [ -d "$SRC_DIR/.git" ]; then
    git -C "$SRC_DIR" fetch origin "$REF"
    git -C "$SRC_DIR" checkout "$REF"
    git -C "$SRC_DIR" pull --ff-only origin "$REF"
    return
  fi

  git clone --branch "$REF" "$REPO_URL" "$SRC_DIR"
}

install_binary() {
  cargo build --release --manifest-path "$SRC_DIR/Cargo.toml"
  mkdir -p "$BIN_DIR"
  install -m 0755 "$SRC_DIR/target/release/pastazzo" "$BIN_PATH"
}

install_extension() {
  mkdir -p "$EXT_DIR"
  find "$EXT_DIR" -mindepth 1 -maxdepth 1 -exec rm -rf {} +
  cp -a "$SRC_DIR/extension/." "$EXT_DIR/"
  glib-compile-schemas "$EXT_DIR/schemas"

  if gnome-extensions info "$OLD_EXT_UUID" >/dev/null 2>&1; then
    gnome-extensions disable "$OLD_EXT_UUID" >/dev/null 2>&1 || true
  fi

  gnome-extensions enable "$EXT_UUID" >/dev/null 2>&1 || true
}

migrate_history() {
  if [ -d "$OLD_DATA_DIR" ] && [ ! -d "$DATA_DIR" ]; then
    mkdir -p "$(dirname "$DATA_DIR")"
    cp -a "$OLD_DATA_DIR" "$DATA_DIR"
  fi
}

main() {
  install_missing_deps
  sync_source
  install_binary
  install_extension
  migrate_history

  echo "Pastazzo installed."
  echo "Shortcut: Shift+Alt+V"
  echo "Required next step: log out and log back in."
  echo "After login, verify or enable it with:"
  echo "  gnome-extensions enable $EXT_UUID"
}

main "$@"

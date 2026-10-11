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
UNIT_DIR="$HOME/.config/systemd/user"

need_cmd() {
  command -v "$1" >/dev/null 2>&1
}

# gnome or wayland. PASTAZZO_SURFACE wins, then --gnome/--wayland, then the session.
choose_surface() {
  if [[ -n ${PASTAZZO_SURFACE:-} ]]; then
    printf '%s\n' "$PASTAZZO_SURFACE"
    return
  fi
  case "${1:-}" in
    --gnome) printf 'gnome\n'; return ;;
    --wayland) printf 'wayland\n'; return ;;
  esac
  local desktop="${XDG_CURRENT_DESKTOP:-} ${XDG_SESSION_DESKTOP:-}"
  if [[ -n ${OMARCHY_PATH:-} || -d /usr/share/omarchy || $desktop == *Hyprland* ]]; then
    printf 'wayland\n'
  else
    printf 'gnome\n'
  fi
}

install_missing_deps() {
  local surface="$1"
  local missing=()

  need_cmd git || missing+=(git)
  need_cmd cargo || missing+=(cargo)
  if [[ $surface == gnome ]]; then
    need_cmd glib-compile-schemas || missing+=(libglib2.0-bin)
    need_cmd gnome-extensions || missing+=(gnome-shell-common)
  else
    need_cmd wl-paste || missing+=(wl-clipboard)
    need_cmd wtype || missing+=(wtype)
  fi

  if [ "${#missing[@]}" -eq 0 ]; then
    return
  fi

  if need_cmd apt-get && need_cmd sudo; then
    echo "Installing missing packages: ${missing[*]}"
    sudo apt-get update
    sudo apt-get install -y "${missing[@]}"
    return
  fi

  if [[ $surface == gnome ]]; then
    echo "Missing required commands. Install git, cargo, glib-compile-schemas and gnome-extensions, then rerun this script." >&2
  else
    echo "Missing required commands. Install git, cargo, wl-clipboard and wtype, then rerun this script." >&2
  fi
  exit 1
}

install_wayland() {
  local plugin_id="turinglabs.pastazzo"
  local plugin_dir="${XDG_CONFIG_HOME:-$HOME/.config}/omarchy/plugins/$plugin_id"
  rm -rf "$plugin_dir"
  mkdir -p "$plugin_dir"
  cp -a "$SRC_DIR/omarchy/." "$plugin_dir/"
  chmod 0755 "$plugin_dir/capture.sh" "$plugin_dir/paste.sh"

  if need_cmd omarchy-shell; then
    omarchy-shell shell rescanPlugins >/dev/null 2>&1 || true
  fi
  if need_cmd omarchy; then
    omarchy plugin enable "$plugin_id" >/dev/null 2>&1 || true
  fi

  local bindings="${XDG_CONFIG_HOME:-$HOME/.config}/hypr/bindings.lua"
  local marker="turinglabs.pastazzo"
  if [[ -f $bindings ]] && ! grep -q "$marker" "$bindings"; then
    cat >>"$bindings" <<'EOF'

-- Pastazzo shelf. Click copies, double-click or Shift+Enter pastes.
o.bind("SHIFT + ALT + V", "Pastazzo", "omarchy-shell shell toggle turinglabs.pastazzo")
EOF
  fi

  local hypr="${XDG_CONFIG_HOME:-$HOME/.config}/hypr/hyprland.lua"
  if [[ -f $hypr ]] && ! grep -q "turinglabs-pastazzo" "$hypr"; then
    cat >>"$hypr" <<'EOF'

-- Pastazzo shelf pops without a layer-shell fade.
hl.layer_rule({ match = { namespace = "^turinglabs-pastazzo$" }, no_anim = true, animation = "none" })
EOF
  fi
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
  cargo build --release --manifest-path "$SRC_DIR/Cargo.toml" -p pastazzo -p pastazzo-sync
  mkdir -p "$BIN_DIR"
  install -m 0755 "$SRC_DIR/target/release/pastazzo" "$BIN_PATH"
  install -m 0755 "$SRC_DIR/target/release/pastazzo-sync" "$BIN_DIR/pastazzo-sync"
}

# Sync only starts once this device joins or logs in to an account: until
# then the .path unit waits and nothing runs.
install_sync_units() {
  need_cmd systemctl || return 0
  mkdir -p "$UNIT_DIR"
  cp "$SRC_DIR/contrib/systemd/pastazzo-sync.path" "$SRC_DIR/contrib/systemd/pastazzo-sync.service" "$UNIT_DIR/"
  systemctl --user daemon-reload >/dev/null 2>&1 || return 0
  systemctl --user enable --now pastazzo-sync.path >/dev/null 2>&1 || true
  # A running sync picks up the new binary.
  systemctl --user try-restart pastazzo-sync.service >/dev/null 2>&1 || true
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
  local surface
  surface=$(choose_surface "${1:-}")
  if [[ ${1:-} == --gnome || ${1:-} == --wayland ]]; then
    shift
  fi

  install_missing_deps "$surface"
  sync_source
  install_binary
  install_sync_units
  migrate_history

  if [[ $surface == wayland ]]; then
    install_wayland
    echo "Pastazzo installed for Hyprland."
    echo "Shortcut: Shift+Alt+V"
    echo "Click copies. Double-click or Shift+Enter pastes."
  else
    install_extension
    echo "Pastazzo installed for GNOME."
    echo "Shortcut: Shift+Alt+V"
    echo "Required next step: log out and log back in."
    echo "After login, verify or enable it with:"
    echo "  gnome-extensions enable $EXT_UUID"
  fi
}

main "$@"

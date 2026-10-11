#!/bin/bash
# wl-paste --watch hands the clipboard to stdin. $1 is the mime we asked for.
set -euo pipefail

mime=${1:-text}
bin=${PASTAZZO_BIN:-$HOME/.local/bin/pastazzo}

# This machine's name, so a local copy is not stored without a source.
# Synced items keep the name of the device that sent them.
device_name() {
  local file=${XDG_CONFIG_HOME:-$HOME/.config}/pastazzo/sync.json
  local name=""
  if [[ -f $file ]]; then
    name=$(sed -n 's/.*"device_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$file" | head -n 1)
  fi
  if [[ -z $name ]]; then
    name=$(hostname -s 2>/dev/null || echo omarchy)
  fi
  printf '%s' "$name"
}

types=$(wl-paste --list-types 2>/dev/null || true)
if [[ ${CLIPBOARD_STATE:-} == sensitive ]] || grep -qx 'x-kde-passwordManagerHint' <<<"$types"; then
  cat >/dev/null
  exit 0
fi

origin=$(device_name)

if [[ $mime == text || $mime == text/* ]]; then
  exec "$bin" add --origin "$origin"
fi

if [[ $mime == image/* ]]; then
  exec "$bin" add-image "$mime" --origin "$origin"
fi

cat >/dev/null

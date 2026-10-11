#!/bin/bash
# Moves a history item to the front and puts it on the Wayland clipboard.
# Usage: paste.sh <id> <kind> <mime> [copy|paste]
set -euo pipefail

id=${1:?}
kind=${2:?}
mime=${3:-text/plain}
mode=${4:-copy}
bin=${PASTAZZO_BIN:-$HOME/.local/bin/pastazzo}

new=$("$bin" touch "$id")
if [[ $kind == image ]]; then
  dir=$("$bin" path)
  file=$(compgen -G "$dir/$new.*" | head -n 1)
  [[ -n $file && -r $file ]]
  wl-copy --type "$mime" <"$file"
else
  "$bin" get "$new" | wl-copy
fi

if [[ $mode == paste ]]; then
  sleep 0.15
  wtype -M shift -k Insert -m shift
fi

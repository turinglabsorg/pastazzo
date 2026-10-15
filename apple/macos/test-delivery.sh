#!/bin/sh
set -eu
ROOT=$(cd "$(dirname "$0")" && pwd)
TEST_BUILD=$(mktemp -d "${TMPDIR:-/tmp}/pastazzo-delivery-tests.XXXXXX")
trap 'rm -rf "$TEST_BUILD"' EXIT
swiftc -parse-as-library \
    "$ROOT/Sources/Pastazzo/Delivery.swift" \
    "$ROOT/Sources/Pastazzo/HistoryStore.swift" \
    "$ROOT/Tests/DeliveryTests.swift" \
    -o "$TEST_BUILD/DeliveryTests"
"$TEST_BUILD/DeliveryTests"
if [ "${1:-}" = "--render" ]; then
    PREVIEW_OUTPUT=${2:?Provide an output directory}
    set --
    for source in "$ROOT"/Sources/Pastazzo/*.swift; do
        case "$source" in */main.swift) continue ;; esac
        set -- "$@" "$source"
    done
    swiftc -O -target "$(uname -m)-apple-macos12.0" -sdk "$(xcrun --show-sdk-path)" \
        -framework Carbon "$@" "$ROOT/Tests/DeliveryPreview.swift" -o "$TEST_BUILD/DeliveryPreview"
    "$TEST_BUILD/DeliveryPreview" "$PREVIEW_OUTPUT"
fi

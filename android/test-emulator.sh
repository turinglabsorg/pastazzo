#!/bin/sh
set -eu
ROOT=$(cd "$(dirname "$0")" && pwd)
TEST_SERIAL=$(adb get-serialno)
case "$TEST_SERIAL" in emulator-*) ;; *) echo 'Select an Android emulator with ANDROID_SERIAL for isolated tests.' >&2; exit 1 ;; esac
"$ROOT/gradlew" -p "$ROOT" :app:assembleDebug :app:assembleDebugAndroidTest :app:lintDebug
adb install -r "$ROOT/app/build/outputs/apk/debug/app-debug.apk"
adb install -r "$ROOT/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk"
adb reverse tcp:32951 tcp:32951
adb reverse tcp:32952 tcp:32952
mkdir -p "$ROOT/build"
adb shell am instrument -w org.pastazzo.android.test/org.pastazzo.android.PastazzoTestRunner > "$ROOT/build/instrumentation.log"
python3 - "$ROOT/build/instrumentation.log" <<'PY'
import re, sys
from pathlib import Path
output = Path(sys.argv[1]).read_text()
match = re.search(r'OK \(\d+ tests?\)', output)
if not match or 'FAILURES!!!' in output:
    print(output)
    raise SystemExit(1)
print(match.group(0))
PY

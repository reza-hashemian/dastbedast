#!/usr/bin/env bash
# Puts this project's own Android pieces into the project that `cargo tauri android init` generates.
# Run from the `app` folder, after init.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
gen="$here/../gen/android/app"
manifest="$gen/src/main/AndroidManifest.xml"

activity="$(find "$gen/src/main" -name MainActivity.kt | head -n 1)"
cp "$here/MainActivity.kt" "$activity"
cp "$here/file_paths.xml" "$gen/src/main/res/xml/file_paths.xml"
cp "$here/dastbedast.pro" "$gen/dastbedast.pro"

if ! grep -q MANAGE_EXTERNAL_STORAGE "$manifest"; then
  python3 - "$manifest" <<'PY'
import sys
path = sys.argv[1]
text = open(path, encoding="utf-8").read()
permissions = '''    <!-- Lets the user choose any folder of the phone for received files. -->
    <uses-permission android:name="android.permission.MANAGE_EXTERNAL_STORAGE" />
    <uses-permission android:name="android.permission.WRITE_EXTERNAL_STORAGE" android:maxSdkVersion="29" />
    <uses-permission android:name="android.permission.READ_EXTERNAL_STORAGE" android:maxSdkVersion="32" />
    <!-- Needed to hear other devices announce themselves on the Wi-Fi network. -->
    <uses-permission android:name="android.permission.CHANGE_WIFI_MULTICAST_STATE" />
'''
assert "<application" in text
text = text.replace("    <application", permissions + "\n    <application\n        android:requestLegacyExternalStorage=\"true\"", 1)
open(path, "w", encoding="utf-8").write(text)
PY
fi
grep -n "uses-permission\|requestLegacy" "$manifest"

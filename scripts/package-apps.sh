#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

workspace_version() {
  awk '
    /^\[workspace.package\]/ { in_workspace = 1; next }
    /^\[/ { in_workspace = 0 }
    in_workspace && $1 == "version" {
      gsub(/"/, "", $3)
      print $3
      exit
    }
  ' "$ROOT_DIR/Cargo.toml"
}

APP_VERSION="${RDL_BUILD_VERSION:-$(workspace_version)}"
if [[ -z "$APP_VERSION" ]]; then
  APP_VERSION="0.0.0"
fi

BUILD_MODE="${1:-release}"
case "$BUILD_MODE" in
  release | --release | -r)
    BUILD_PROFILE="release"
    CARGO_PROFILE_ARGS=(--release)
    TARGET_PROFILE_DIR="release"
    ;;
  debug | --debug)
    BUILD_PROFILE="debug"
    CARGO_PROFILE_ARGS=()
    TARGET_PROFILE_DIR="debug"
    ;;
  -h | --help)
    echo "Usage: $0 [debug|release|--debug|--release|-r]"
    echo
    echo "Packages Rust Desk Light Client and Admin as separate local app directories."
    exit 0
    ;;
  *)
    echo "Unknown build mode: $BUILD_MODE" >&2
    echo "Usage: $0 [debug|release|--debug|--release|-r]" >&2
    exit 2
    ;;
esac

platform_name() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$arch" in
    x86_64 | amd64) arch="x64" ;;
    arm64 | aarch64) arch="arm64" ;;
  esac

  case "$os" in
    Darwin) echo "macos-$arch" ;;
    Linux) echo "linux-$arch" ;;
    *)
      echo "Unsupported platform: $os" >&2
      exit 1
      ;;
  esac
}

reset_dir() {
  local path="$1"
  rm -rf "$path"
  mkdir -p "$path"
}

copy_config_templates() {
  local dest="$1"
  if [[ -d "$ROOT_DIR/config" ]]; then
    cp -R "$ROOT_DIR/config" "$dest/config"
  fi
}

copy_i18n() {
  local dest="$1"
  if [[ -d "$ROOT_DIR/assets/i18n" ]]; then
    cp -R "$ROOT_DIR/assets/i18n" "$dest/i18n"
  fi
}

write_macos_plist() {
  local plist="$1"
  local bundle_id="$2"
  local display_name="$3"
  local executable="$4"
  local usage_keys="$5"

  cat >"$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleExecutable</key>
    <string>$executable</string>
    <key>CFBundleIconFile</key>
    <string>rdl-icon</string>
    <key>CFBundleIdentifier</key>
    <string>$bundle_id</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>$display_name</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$APP_VERSION</string>
    <key>CFBundleVersion</key>
    <string>$APP_VERSION</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.15</string>
$usage_keys
</dict>
</plist>
EOF
}

package_macos_app() {
  local display_name="$1"
  local package_slug="$2"
  local binary_name="$3"
  local bundle_id="$4"
  local usage_keys="$5"
  local platform_dir="$ROOT_DIR/dist/apps/$(platform_name)"
  local app_dir="$platform_dir/$display_name.app"
  local macos_dir="$app_dir/Contents/MacOS"
  local resources_dir="$app_dir/Contents/Resources"
  local bin_source="$ROOT_DIR/target/$TARGET_PROFILE_DIR/$binary_name"
  local icon_source="$ROOT_DIR/assets/icons/rdl-icon.icns"
  local zip_path="$platform_dir/$package_slug-$(platform_name).zip"

  [[ -f "$bin_source" ]] || { echo "Missing built executable: $bin_source" >&2; exit 1; }
  [[ -f "$icon_source" ]] || { echo "Missing icon: $icon_source" >&2; exit 1; }

  reset_dir "$app_dir"
  mkdir -p "$macos_dir" "$resources_dir"
  cp "$bin_source" "$macos_dir/$binary_name"
  chmod +x "$macos_dir/$binary_name"
  cp "$icon_source" "$resources_dir/rdl-icon.icns"
  copy_config_templates "$resources_dir"
  if [[ "$binary_name" == "rdl-admin-gui" ]]; then
    copy_i18n "$macos_dir"
  fi
  printf '%s\n\nDouble-click this app to start it. It does not require a terminal window.\n' "$display_name" >"$resources_dir/README.txt"
  write_macos_plist "$app_dir/Contents/Info.plist" "$bundle_id" "$display_name" "$binary_name" "$usage_keys"

  if command -v codesign >/dev/null 2>&1; then
    codesign --force --sign - --identifier "$bundle_id" "$app_dir"
    codesign --verify --verbose "$app_dir"
  fi

  rm -f "$zip_path"
  if command -v ditto >/dev/null 2>&1; then
    ditto -c -k --keepParent "$app_dir" "$zip_path"
  else
    (cd "$platform_dir" && zip -qr "$zip_path" "$(basename "$app_dir")")
  fi

  echo "Packaged $display_name"
  echo "  App:     $app_dir"
  echo "  Archive: $zip_path"
}

write_desktop_file() {
  local desktop_path="$1"
  local display_name="$2"
  local app_run_name="$3"

  cat >"$desktop_path" <<EOF
[Desktop Entry]
Type=Application
Name=$display_name
Exec=sh -c 'cd "\$(dirname "\$1")" && exec ./$app_run_name' rdl-launcher %k
Icon=rdl-icon
Terminal=false
Categories=Network;RemoteAccess;
EOF
}

write_app_run() {
  local app_run_path="$1"
  local binary_name="$2"

  cat >"$app_run_path" <<EOF
#!/usr/bin/env bash
set -euo pipefail
APP_DIR="\$(cd "\$(dirname "\${BASH_SOURCE[0]}")" && pwd)"
exec "\$APP_DIR/$binary_name" "\$@"
EOF
  chmod +x "$app_run_path"
}

package_linux_appdir() {
  local display_name="$1"
  local package_slug="$2"
  local binary_name="$3"
  local desktop_name="$4"
  local platform_dir="$ROOT_DIR/dist/apps/$(platform_name)"
  local app_dir="$platform_dir/$display_name.AppDir"
  local bin_source="$ROOT_DIR/target/$TARGET_PROFILE_DIR/$binary_name"
  local icon_source="$ROOT_DIR/assets/icons/rdl-icon-256.png"
  local archive_path="$platform_dir/$package_slug-$(platform_name).tar.gz"

  [[ -f "$bin_source" ]] || { echo "Missing built executable: $bin_source" >&2; exit 1; }
  [[ -f "$icon_source" ]] || { echo "Missing icon: $icon_source" >&2; exit 1; }

  reset_dir "$app_dir"
  mkdir -p "$app_dir/usr/share/icons/hicolor/256x256/apps"
  cp "$bin_source" "$app_dir/$binary_name"
  chmod +x "$app_dir/$binary_name"
  cp "$icon_source" "$app_dir/rdl-icon.png"
  cp "$icon_source" "$app_dir/.DirIcon"
  cp "$icon_source" "$app_dir/usr/share/icons/hicolor/256x256/apps/rdl-icon.png"
  copy_config_templates "$app_dir"
  if [[ "$binary_name" == "rdl-admin-gui" ]]; then
    copy_i18n "$app_dir"
  fi
  write_app_run "$app_dir/AppRun" "$binary_name"
  write_desktop_file "$app_dir/$desktop_name.desktop" "$display_name" "AppRun"
  printf '%s\n\nDouble-click AppRun or the GUI binary to start the app. Terminal=false is set in the desktop entry, and icons are included for AppDir/AppImage-compatible launchers.\n' "$display_name" >"$app_dir/README.txt"

  rm -f "$archive_path"
  tar -czf "$archive_path" -C "$platform_dir" "$(basename "$app_dir")"

  echo "Packaged $display_name"
  echo "  AppDir:  $app_dir"
  echo "  Archive: $archive_path"
}

echo "Building Rust Desk Light Client ($BUILD_PROFILE)"
cargo build --manifest-path "$ROOT_DIR/Cargo.toml" -p rust-desk-light-client --bin rdl-client-gui "${CARGO_PROFILE_ARGS[@]}"

echo "Building Rust Desk Light Admin ($BUILD_PROFILE)"
cargo build --manifest-path "$ROOT_DIR/Cargo.toml" -p rust-desk-light-admin --bin rdl-admin-gui "${CARGO_PROFILE_ARGS[@]}"

case "$(uname -s)" in
  Darwin)
    CLIENT_USAGE='    <key>NSCameraUsageDescription</key>
    <string>Rust Desk Light uses the camera when an admin starts camera live control.</string>
    <key>NSMicrophoneUsageDescription</key>
    <string>Rust Desk Light uses the microphone for voice chat when enabled.</string>'
    ADMIN_USAGE='    <key>NSMicrophoneUsageDescription</key>
    <string>Rust Desk Light Admin uses the microphone for voice chat when enabled.</string>'

    package_macos_app "Rust Desk Light Client" "Rust-Desk-Light-Client" "rdl-client-gui" "local.rust-desk-light.client" "$CLIENT_USAGE"
    package_macos_app "Rust Desk Light Admin" "Rust-Desk-Light-Admin" "rdl-admin-gui" "local.rust-desk-light.admin.gui" "$ADMIN_USAGE"
    ;;
  Linux)
    package_linux_appdir "Rust Desk Light Client" "Rust-Desk-Light-Client" "rdl-client-gui" "rust-desk-light-client"
    package_linux_appdir "Rust Desk Light Admin" "Rust-Desk-Light-Admin" "rdl-admin-gui" "rust-desk-light-admin"
    ;;
  *)
    echo "Unsupported platform for this script." >&2
    exit 1
    ;;
esac

#!/usr/bin/env bash
# Build, ad-hoc sign and package the macOS app without requiring a Developer ID.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

APP_NAME="Yuetu"
BUNDLE_DIR="$ROOT/target/release/bundle/osx"
DMG_DIR="$ROOT/target/release/bundle/dmg"
STAGING_DIR="$DMG_DIR/.staging"
APP_PATH="$BUNDLE_DIR/$APP_NAME.app"
DMG_PATH="$DMG_DIR/$APP_NAME.dmg"
VERSION="${YUETU_VERSION:-}"

cargo build --release -p yuetu-app
cargo bundle --release --format osx -p yuetu-app

if [[ ! -d "$APP_PATH" ]]; then
    echo "cargo-bundle did not create $APP_PATH" >&2
    exit 1
fi

if [[ -n "$VERSION" ]]; then
    /usr/libexec/PlistBuddy \
        -c "Set :CFBundleShortVersionString $VERSION" \
        "$APP_PATH/Contents/Info.plist"
fi

# cargo-bundle leaves the app with only the linker's partial signature. Re-sign
# the whole bundle so it survives Gatekeeper's ad-hoc verification.
codesign --force --deep --sign - "$APP_PATH"
xattr -cr "$APP_PATH"
codesign --verify --deep --strict --verbose=2 "$APP_PATH"

rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR"
cp -R "$APP_PATH" "$STAGING_DIR/$APP_NAME.app"
ln -s /Applications "$STAGING_DIR/Applications"

rm -f "$DMG_PATH"
hdiutil create \
    -volname "$APP_NAME" \
    -srcfolder "$STAGING_DIR" \
    -ov \
    -format UDZO \
    "$DMG_PATH"
rm -rf "$STAGING_DIR"

echo "packaged: $DMG_PATH"

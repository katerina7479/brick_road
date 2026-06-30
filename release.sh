#!/usr/bin/env bash
#
# release.sh — build, bundle, and (optionally) publish a macOS release of Brick Road.
#
# Usage:
#   ./release.sh                  Build the .app and zip it into dist/.
#   ./release.sh --publish        Also create a GitHub release and upload the zip.
#   ./release.sh --publish 0.2.0  Publish with an explicit version/tag instead of
#                                 the one in Cargo.toml.
#
# The build is UNSIGNED. A recipient on another Mac must strip the Gatekeeper
# quarantine flag once after unzipping (the release notes spell this out):
#     xattr -cr "/Applications/Brick Road.app"
#
# Note: the binary is built for THIS machine's CPU architecture only. To ship to
# both Intel and Apple Silicon you'd need a universal build (not done here).

set -euo pipefail
cd "$(dirname "$0")"

APP_NAME="Brick Road"
DIST="dist"

# Version: 2nd CLI arg if given, else the [package] version in Cargo.toml.
VERSION="${2:-$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')}"
TAG="v${VERSION#v}"
ZIP="$DIST/BrickRoad-$TAG.zip"
APP_PATH="target/release/bundle/osx/$APP_NAME.app"

echo "==> Building $APP_NAME $TAG (release)…"
command -v cargo-bundle >/dev/null 2>&1 || cargo install cargo-bundle
cargo bundle --release

[ -d "$APP_PATH" ] || { echo "error: $APP_PATH not found after bundle" >&2; exit 1; }

echo "==> Zipping…"
mkdir -p "$DIST"
rm -f "$ZIP"
# `ditto` preserves macOS bundle structure/metadata better than `zip`.
ditto -c -k --sequesterRsrc --keepParent "$APP_PATH" "$ZIP"
echo "    created $ZIP ($(du -h "$ZIP" | cut -f1))"

if [ "${1:-}" = "--publish" ]; then
    echo "==> Publishing GitHub release ${TAG}…"
    NOTES="$(cat <<EOF
**Brick Road $TAG** — macOS (unsigned, built for \`$(uname -m)\`).

**Install**
1. Unzip and move \`$APP_NAME.app\` to /Applications.
2. This build isn't code-signed, so macOS quarantines it. Run once:
   \`\`\`
   xattr -cr "/Applications/$APP_NAME.app"
   \`\`\`
3. Open it normally.

Your data lives in \`~/Library/Application Support/\`, so replacing the app with a
newer build never touches it.
EOF
)"
    # Create the release if the tag is new; otherwise just upload/replace the asset.
    if gh release view "$TAG" >/dev/null 2>&1; then
        gh release upload "$TAG" "$ZIP" --clobber
    else
        gh release create "$TAG" "$ZIP" --title "$APP_NAME $TAG" --notes "$NOTES"
    fi
    echo "    published $TAG."
else
    echo "==> Done. Zip is at $ZIP"
    echo "    To publish a GitHub release: ./release.sh --publish"
fi

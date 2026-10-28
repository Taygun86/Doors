#!/usr/bin/env bash
set -euo pipefail

APP_ID="io.github.doors.Doors"
REPO_URL="https://taygun86.github.io/taygun86-repo/flatpak"
BUILD_DIR="build-flatpak"
REPO_DIR="flatpak"

echo "==> [1/5] Building Doors release binary..."
cargo build --release

# Detect installed org.gnome.Platform branch (fallback to 48)
GNOME_BRANCH=$(flatpak list --runtime --columns=application,branch 2>/dev/null | awk '$1 == "org.gnome.Platform" {print $2}' | sort -V | tail -n1)
if [ -z "${GNOME_BRANCH:-}" ]; then
    GNOME_BRANCH="48"
fi
echo "==> [2/5] Initializing Flatpak build tree (org.gnome.Platform//${GNOME_BRANCH})..."

rm -rf "$BUILD_DIR"
flatpak build-init "$BUILD_DIR" "$APP_ID" org.gnome.Platform org.gnome.Platform "$GNOME_BRANCH"

mkdir -p "$BUILD_DIR/files/bin" \
         "$BUILD_DIR/files/share/applications" \
         "$BUILD_DIR/files/share/icons/hicolor/scalable/apps" \
         "$BUILD_DIR/files/share/icons/hicolor/512x512/apps" \
         "$BUILD_DIR/files/share/locale"

install -Dm755 target/release/doors "$BUILD_DIR/files/bin/doors"
install -Dm644 data/io.github.doors.Doors.desktop "$BUILD_DIR/files/share/applications/io.github.doors.Doors.desktop"
install -Dm644 data/io.github.doors.Doors.svg "$BUILD_DIR/files/share/icons/hicolor/scalable/apps/io.github.doors.Doors.svg"
install -Dm644 data/io.github.doors.Doors.png "$BUILD_DIR/files/share/icons/hicolor/512x512/apps/io.github.doors.Doors.png"
cp -r locale/* "$BUILD_DIR/files/share/locale/"

echo "==> [3/5] Finishing Flatpak permissions..."
flatpak build-finish "$BUILD_DIR" \
    --command=doors \
    --share=ipc \
    --socket=fallback-x11 \
    --socket=wayland \
    --socket=pulseaudio \
    --device=all \
    --filesystem=/mnt \
    --filesystem=/run/media \
    --filesystem=host \
    --filesystem=~/.local/share/Steam/compatibilitytools.d:ro \
    --filesystem=~/.var/app/com.valvesoftware.Steam/.local/share/Steam/compatibilitytools.d:ro \
    --filesystem=~/.local/share/applications:create \
    --filesystem=~/.local/share/icons:create \
    --talk-name=org.freedesktop.Flatpak

GPG_ARGS=()
if [ -n "${GPG_KEY_ID:-}" ]; then
    GPG_ARGS+=(--gpg-sign="$GPG_KEY_ID")
fi

echo "==> [4/5] Exporting to OSTree repository (./${REPO_DIR})..."
flatpak build-export "${GPG_ARGS[@]}" "$REPO_DIR" "$BUILD_DIR" stable
flatpak build-update-repo "${GPG_ARGS[@]}" --title="Taygun86 Flatpak Repository" --generate-static-deltas "$REPO_DIR"

echo "==> [5/5] Generating taygun86.flatpakrepo and doors.flatpak bundle..."
cat > "$REPO_DIR/taygun86.flatpakrepo" <<EOF
[Flatpak Repo]
Title=Taygun86 Repository
Url=${REPO_URL}
Homepage=https://github.com/Taygun86/taygun86-repo
Comment=Taygun86 Flatpak Repository for Linux applications
DefaultBranch=stable
NoDeps=false
EOF

flatpak build-bundle "$REPO_DIR" "$REPO_DIR/doors.flatpak" "$APP_ID" stable --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo

rm -rf "$BUILD_DIR"

echo ""
echo "=================================================================="
echo "Flatpak repository generated in: ./${REPO_DIR}/"
echo ""
echo "To publish to taygun86-repo (GitHub Pages):"
echo "  1. Copy the './${REPO_DIR}' folder into the root of 'taygun86-repo'"
echo "     (next to 'deb/' and 'rpm/')."
echo "  2. Ensure an empty '.nojekyll' file exists in the root of 'taygun86-repo':"
echo "     touch .nojekyll"
echo "  3. Commit and push to GitHub."
echo ""
echo "Users can then add your 3rd-party Flatpak repo & install Doors via:"
echo "  flatpak remote-add --if-not-exists --no-gpg-verify taygun86 ${REPO_URL}"
echo "  flatpak install taygun86 ${APP_ID}"
echo "=================================================================="


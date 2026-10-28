#!/usr/bin/env bash
set -euo pipefail

APP_ID="io.github.doors.Doors"
REPO_URL="https://taygun86.github.io/taygun86-repo/flatpak"
BUILD_DIR="build-flatpak"
REPO_DIR="flatpak"
GPG_DIR=".flatpak-gpg"

echo "==> [1/6] Ensuring GPG signing key in ./${GPG_DIR}..."
mkdir -p "$GPG_DIR"
chmod 700 "$GPG_DIR"

if ! gpg --homedir "$GPG_DIR" --list-secret-keys "repo@taygun86.github.io" >/dev/null 2>&1; then
    gpg --homedir "$GPG_DIR" --batch --passphrase "" --quick-generate-key \
        "Taygun86 Flatpak Repo <repo@taygun86.github.io>" rsa3072 sign never
fi

KEY_ID=$(gpg --homedir "$GPG_DIR" --list-secret-keys --with-colons "repo@taygun86.github.io" | awk -F: '$1 == "sec" {print $5; exit}')
gpg --homedir "$GPG_DIR" --export "$KEY_ID" > "$GPG_DIR/taygun86.gpg"
GPG_BASE64=$(base64 -w0 "$GPG_DIR/taygun86.gpg")

echo "==> [2/6] Building Doors release binary..."
cargo build --release

# Detect installed org.gnome.Platform branch (fallback to 48)
GNOME_BRANCH=$(flatpak list --runtime --columns=application,branch 2>/dev/null | awk '$1 == "org.gnome.Platform" {print $2}' | sort -V | tail -n1)
if [ -z "${GNOME_BRANCH:-}" ]; then
    GNOME_BRANCH="48"
fi
echo "==> [3/6] Initializing Flatpak build tree (org.gnome.Platform//${GNOME_BRANCH})..."

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

echo "==> [4/6] Finishing Flatpak permissions..."
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

echo "==> [5/6] Exporting and GPG-signing OSTree repository (./${REPO_DIR})..."
rm -rf "$REPO_DIR"
flatpak build-export --gpg-homedir="$GPG_DIR" --gpg-sign="$KEY_ID" "$REPO_DIR" "$BUILD_DIR" stable
flatpak build-update-repo --gpg-homedir="$GPG_DIR" --gpg-sign="$KEY_ID" --title="Taygun86 Flatpak Repository" --generate-static-deltas "$REPO_DIR"

echo "==> [6/6] Generating GPG-verified taygun86.flatpakrepo and doors.flatpak bundle..."
cat > "$REPO_DIR/taygun86.flatpakrepo" <<EOF
[Flatpak Repo]
Title=Taygun86 Repository
Url=${REPO_URL}
Homepage=https://github.com/Taygun86/taygun86-repo
Comment=Taygun86 Flatpak Repository for Linux applications
DefaultBranch=stable
GPGKey=${GPG_BASE64}
EOF

cp -f "$GPG_DIR/taygun86.gpg" "$REPO_DIR/taygun86.gpg"

flatpak build-bundle \
    --repo-url="${REPO_URL}" \
    --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo \
    --gpg-keys="$GPG_DIR/taygun86.gpg" \
    "$REPO_DIR" "doors.flatpak" "$APP_ID" stable
cp -f "doors.flatpak" "$REPO_DIR/doors.flatpak"

rm -rf "$BUILD_DIR"

if [ -d "../taygun86-repo" ]; then
    echo "==> Syncing ./${REPO_DIR} into ../taygun86-repo/flatpak..."
    rm -rf "../taygun86-repo/flatpak"
    cp -r "$REPO_DIR" "../taygun86-repo/flatpak"
    touch "../taygun86-repo/.nojekyll"
    echo "==> Committing and pushing ../taygun86-repo ('flatpak update')..."
    (
        cd "../taygun86-repo"
        git add .
        git commit -m "flatpak update" || echo "No new changes to commit."
        git push
    )
    echo "==> ../taygun86-repo/flatpak automatically updated and pushed!"
fi

echo ""
echo "=================================================================="
echo "GPG-signed Flatpak repository generated in: ./${REPO_DIR}/"
echo ""
echo "Users can add your 3rd-party Flatpak repo & install Doors via:"
echo "  flatpak remote-add --if-not-exists taygun86 ${REPO_URL}/taygun86.flatpakrepo"
echo "  flatpak install taygun86 ${APP_ID}"
echo "=================================================================="

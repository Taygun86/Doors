#!/usr/bin/env bash
set -euo pipefail

if command -v msgfmt >/dev/null 2>&1; then
    for po_file in po/*.po; do
        [ -f "$po_file" ] || continue
        lang="$(basename "$po_file" .po)"
        mkdir -p "locale/$lang/LC_MESSAGES"
        msgfmt "$po_file" -o "locale/$lang/LC_MESSAGES/doors.mo"
    done
fi

echo "==> Building Doors (Release)..."
cargo build --release --offline 2>/dev/null || cargo build --release

mkdir -p "$HOME/.local/bin"
mkdir -p "$HOME/.local/share/applications"
mkdir -p "$HOME/.local/share/icons/hicolor/scalable/apps"
mkdir -p "$HOME/.local/share/icons/hicolor/512x512/apps"

install -Dm755 target/release/doors "$HOME/.local/bin/doors"
install -Dm644 data/io.github.doors.Doors.svg "$HOME/.local/share/icons/hicolor/scalable/apps/io.github.doors.Doors.svg"
install -Dm644 data/io.github.doors.Doors.png "$HOME/.local/share/icons/hicolor/512x512/apps/io.github.doors.Doors.png"

for mo_file in locale/*/LC_MESSAGES/doors.mo; do
    [ -f "$mo_file" ] || continue
    lang="$(basename "$(dirname "$(dirname "$mo_file")")")"
    install -Dm644 "$mo_file" "$HOME/.local/share/locale/$lang/LC_MESSAGES/doors.mo"
done

sed "s|Exec=doors %f|Exec=\"$HOME/.local/bin/doors\" %f|g" data/io.github.doors.Doors.desktop \
    > "$HOME/.local/share/applications/io.github.doors.Doors.desktop"
chmod 755 "$HOME/.local/share/applications/io.github.doors.Doors.desktop"

if command -v xdg-mime >/dev/null 2>&1; then
    xdg-mime default io.github.doors.Doors.desktop application/x-ms-dos-executable || true
    xdg-mime default io.github.doors.Doors.desktop application/x-msdownload || true
    xdg-mime default io.github.doors.Doors.desktop application/vnd.microsoft.portable-executable || true
    xdg-mime default io.github.doors.Doors.desktop application/x-msi || true
    xdg-mime default io.github.doors.Doors.desktop application/x-ms-installer || true
    xdg-mime default io.github.doors.Doors.desktop application/vnd.ms-msi || true
fi

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$HOME/.local/share/applications" || true
fi

echo "==> Installation complete!"

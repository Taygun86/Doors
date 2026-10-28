#!/usr/bin/env bash
set -euo pipefail

echo "==> Removing Doors..."

rm -f "$HOME/.local/bin/doors"
rm -f "$HOME/.local/share/applications/io.github.doors.Doors.desktop"
rm -f "$HOME/.local/share/icons/hicolor/scalable/apps/io.github.doors.Doors.svg"
rm -f "$HOME/.local/share/icons/hicolor/512x512/apps/io.github.doors.Doors.png"
rm -f "$HOME/.local/share/locale/"*/LC_MESSAGES/doors.mo

if command -v flatpak >/dev/null 2>&1; then
    if flatpak info --user io.github.doors.Doors >/dev/null 2>&1; then
        flatpak uninstall --user -y io.github.doors.Doors || true
    fi
fi

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$HOME/.local/share/applications" || true
fi

echo "==> Doors removed."

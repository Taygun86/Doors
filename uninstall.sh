#!/usr/bin/env bash
set -euo pipefail

echo "==> Removing Doors..."

rm -f "$HOME/.local/bin/doors"
rm -f "$HOME/.local/share/applications/io.github.Taygun86.Doors.desktop"
rm -f "$HOME/.local/share/applications/io.github.doors.Doors.desktop"
rm -f "$HOME/.local/share/icons/hicolor/scalable/apps/io.github.Taygun86.Doors.svg"
rm -f "$HOME/.local/share/icons/hicolor/scalable/apps/io.github.doors.Doors.svg"
rm -f "$HOME/.local/share/icons/hicolor/512x512/apps/io.github.Taygun86.Doors.png"
rm -f "$HOME/.local/share/icons/hicolor/512x512/apps/io.github.doors.Doors.png"
rm -f "$HOME/.local/share/locale/"*/LC_MESSAGES/doors.mo
rm -f "$HOME/.config/autostart/io.github.Taygun86.Doors.updater.desktop"
rm -f "$HOME/.config/autostart/io.github.doors.Doors.updater.desktop"

if command -v systemctl >/dev/null 2>&1; then
    systemctl --user disable --now doors-updater.timer >/dev/null 2>&1 || true
    rm -f "$HOME/.config/systemd/user/doors-updater.timer"
    rm -f "$HOME/.config/systemd/user/doors-updater.service"
    systemctl --user daemon-reload >/dev/null 2>&1 || true
else
    rm -f "$HOME/.config/systemd/user/doors-updater.timer"
    rm -f "$HOME/.config/systemd/user/doors-updater.service"
fi

if command -v flatpak >/dev/null 2>&1; then
    for app_id in io.github.Taygun86.Doors io.github.doors.Doors; do
        if flatpak info --user "$app_id" >/dev/null 2>&1; then
            flatpak uninstall --user -y "$app_id" || true
        fi
    done
fi

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$HOME/.local/share/applications" || true
fi

echo "==> Doors removed."

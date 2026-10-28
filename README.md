<div align="center">
  <img src="data/io.github.doors.Doors.svg" width="128" height="128" alt="Doors Icon" />
  <h1>Doors</h1>
  <p><b>A fast, minimal GTK4 / Libadwaita Windows (<code>.exe</code>) program installer and runner for Linux powered by Proton.</b></p>
</div>

---

## Overview

**Doors** (`io.github.doors.Doors`) lets you double-click any Windows `.exe` file in your Linux file manager, choose a target disk and a Proton version, and click **Install**. Doors automatically creates an isolated Proton prefix (`pfx`), handles setup installers or portable program folders, extracts the embedded `.exe` icon, and adds a native application shortcut (`.desktop`) to your desktop environment's app menu.

---

## Proton & `compatibilitytools.d` Requirement

Doors does not bundle its own Wine or Proton runtime. Instead, it automatically discovers custom Proton and Wine runners installed in your system's `compatibilitytools.d` directories:

- `~/.local/share/Steam/compatibilitytools.d`
- `~/.steam/root/compatibilitytools.d`
- `~/.steam/steam/compatibilitytools.d`
- `~/.var/app/com.valvesoftware.Steam/.local/share/Steam/compatibilitytools.d`
- `/usr/share/steam/compatibilitytools.d`
- `/usr/local/share/steam/compatibilitytools.d`
- Custom paths via the `DOORS_COMPAT_TOOLS_PATH` environment variable (colon-separated)

### Installing Proton Versions with ProtonPlus (Recommended)

The easiest way to download, update, and manage Proton versions (such as **GE-Proton**, **Proton-CachyOS**, **Proton-Sarek**, or **Wine-GE**) in `compatibilitytools.d` is **[ProtonPlus](https://github.com/Vysp3r/ProtonPlus)**:

```bash
flatpak install flathub com.vysp3r.ProtonPlus
```

1. Open **ProtonPlus**.
2. Select **Steam** as the launcher target (which installs runners into `compatibilitytools.d`).
3. Download your preferred Proton build (e.g., **GE-Proton Latest**).
4. Launch **Doors** — your installed Proton versions will appear automatically in the Proton selector.

---

## Features

- **Double-Click `.exe` Wizard:** Associate `.exe` files with Doors to get a clean install wizard showing the extracted `.exe` icon, program name, target disk, Proton version, and an optional *"Run after installation"* switch.
- **Smart Installer & Portable Folder Detection:**
  - **Portable / Pre-installed Folders:** Automatically detects when an `.exe` is inside a portable program directory, moves the entire folder to `<Disk>/Doors/<Program>/game/` (instant rename on the same drive, or byte-level progress across drives), and configures the prefix.
  - **Setup Installers (`setup.exe`, `*install*.exe`, repacks):** Runs the installer inside `<Disk>/Doors/<Program>/pfx`, then scans Windows Desktop/Start Menu `.lnk` shortcuts and `pfx/drive_c` to automatically bind the installed main executable or launcher.
  - **Single Standalone `.exe`:** Runs standalone executables inside an isolated prefix and moves portable single-file programs into `<Disk>/Doors/<Program>/game/`.
- **Multi-Disk Support:** Automatically detects your main disk (`~/Doors`) as well as secondary drives mounted under `/mnt/*`, `/run/media/$USER/*`, and `/media/$USER/*` with available free space indicators.
- **Automatic Icon & `.desktop` Shortcut Generation:** Parses PE resources directly from the `.exe` to extract the highest-resolution icon (`.ico` / PNG) and creates a desktop entry in `~/.local/share/applications/`.
- **Per-Program Settings & Prefix Management:**
  - Switch Proton version at any time
  - Custom launch arguments / environment variables (supports `%command%`)
  - Manually change the target main `.exe`
  - **Synchronization:** `NTSYNC` (auto-selected when `/dev/ntsync` is available), `FSYNC`, `ESYNC`, or Disabled
  - **Display & Graphics:** Native Wayland (`PROTON_ENABLE_WAYLAND=1`), HDR (`PROTON_ENABLE_HDR=1`), Discrete GPU offload (`DRI_PRIME=1` & NVIDIA Prime), NVIDIA DLSS / NVAPI (`PROTON_ENABLE_NVAPI=1`), AMD FSR (`WINE_FULLSCREEN_FSR=1`), and **vkBasalt**
  - **Performance & Overlay:** **Feral GameMode** (auto-enabled if installed), **MangoHud**, and **Gamescope** (with custom resolution, FPS limit, and fullscreen controls)
  - **Prefix Tools:** Open program/prefix folder, run a patch/update `.exe` inside the prefix, open `winecfg` or `Winetricks`, or **Reset Prefix** (`pfx`) without deleting program files.
- **Multilingual UI:** English default in source code with built-in translations for 25 additional languages (`tr`, `de`, `fr`, `es`, `it`, `pt`, `ru`, `uk`, `pl`, `nl`, `cs`, `hu`, `ro`, `el`, `sv`, `az`, `kk`, `ar`, `fa`, `hi`, `id`, `vi`, `ja`, `ko`, `zh`).

---

## Building & Installation

### Dependencies

Make sure you have the Rust toolchain (`cargo`, `rustc`), `gettext` (`msgfmt`), and the development packages for **GTK4** and **Libadwaita** installed:

- **Fedora:**
  ```bash
  sudo dnf install cargo rust gtk4-devel libadwaita-devel gettext
  ```
- **Ubuntu / Debian:**
  ```bash
  sudo apt install cargo rustc libgtk-4-dev libadwaita-1-dev gettext
  ```
- **Arch Linux:**
  ```bash
  sudo pacman -S rust gtk4 libadwaita gettext
  ```

### System / User Installation

Run the included install script to build the release binary and install the desktop entry, icons, MIME associations, and locale files:

```bash
./install.sh
```

To uninstall:

```bash
./uninstall.sh
```

### Flatpak (Taygun86 Repository)

You can install **Doors** directly from the **Taygun86** 3rd-party Flatpak repository hosted on GitHub Pages:

```bash
flatpak remote-add --if-not-exists taygun86 https://taygun86.github.io/taygun86-repo/flatpak/taygun86.flatpakrepo
flatpak install taygun86 io.github.doors.Doors
```

### Local Flatpak Build & Repository Export

To generate the static OSTree Flatpak repository (`./flatpak/`) and single-file bundle (`./flatpak/doors.flatpak`) for publishing to `taygun86-repo`:

```bash
./build-flatpak-repo.sh
```

Or build and install locally using `flatpak-builder` ([`io.github.doors.Doors.yml`](io.github.doors.Doors.yml)):

```bash
flatpak-builder --user --install --force-clean build-dir io.github.doors.Doors.yml
```

---

## License

This project is licensed under the **GNU General Public License v3.0 or later (GPL-3.0-or-later)** — see the [LICENSE](LICENSE) file for details.



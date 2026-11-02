use crate::detector::{DoorsGameConfig, SyncMode};
use crate::disks::real_user_home;
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[derive(Debug, Clone)]
pub struct ProtonTool {
    /// Folder name inside compatibilitytools.d, e.g. "GE-Proton9-20"
    pub name: String,
    /// Full directory path, e.g. "/home/user/.local/share/Steam/compatibilitytools.d/GE-Proton9-20"
    pub dir_path: PathBuf,
    /// Path to `proton` script or `wine` binary
    pub executable: PathBuf,
    /// True if using Valve/GE `proton` script, false if using direct `wine` binary
    pub is_proton_script: bool,
}

/// Check if a command binary (such as `gamemoderun`, `mangohud`, `gamescope`, `winetricks`) is installed on the system/host.
pub fn has_host_binary(bin_name: &str) -> bool {
    let home = real_user_home();
    let fixed_dirs = [
        PathBuf::from("/usr/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/bin"),
        PathBuf::from("/var/run/host/usr/bin"),
        PathBuf::from("/var/run/host/usr/local/bin"),
        PathBuf::from("/var/run/host/bin"),
        home.join(".local/bin"),
    ];

    for dir in &fixed_dirs {
        let candidate = dir.join(bin_name);
        if is_executable_file(&candidate) {
            return true;
        }
    }

    if let Ok(path_env) = std::env::var("PATH") {
        for part in path_env.split(':') {
            if !part.is_empty() {
                let candidate = Path::new(part).join(bin_name);
                if is_executable_file(&candidate) {
                    return true;
                }
            }
        }
    }

    false
}

/// Check if vkBasalt Vulkan layer is installed on the system/host.
pub fn has_vkbasalt() -> bool {
    let candidates = [
        "/usr/share/vulkan/implicit_layer.d/vkBasalt.json",
        "/usr/local/share/vulkan/implicit_layer.d/vkBasalt.json",
        "/usr/lib64/libvkbasalt.so",
        "/usr/lib/libvkbasalt.so",
        "/var/run/host/usr/share/vulkan/implicit_layer.d/vkBasalt.json",
        "/var/run/host/usr/lib64/libvkbasalt.so",
        "/var/run/host/usr/lib/libvkbasalt.so",
    ];
    candidates.iter().any(|p| Path::new(p).exists()) || has_host_binary("vkbasalt")
}

/// Check if the system has an AMD GPU with RDNA 2, RDNA 3, or RDNA 4 architecture
/// which officially supports FSR 4. Cached per process so `lspci` never blocks the GTK UI repeatedly.
pub fn has_fsr4_supported_gpu() -> bool {
    static CACHED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHED.get_or_init(detect_fsr4_supported_gpu)
}

fn detect_fsr4_supported_gpu() -> bool {
    // 1. Try checking via lspci
    let mut cmd = crate::proton_manager::build_command("lspci");
    if let Ok(output) = cmd.output() {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).to_lowercase();
            for line in stdout.lines() {
                if line.contains("vga compatible controller")
                    || line.contains("3d controller")
                    || line.contains("display controller")
                {
                    if line.contains("advanced micro devices")
                        || line.contains("amd")
                        || line.contains("ati")
                    {
                        if line.contains("navi 2")
                            || line.contains("navi 3")
                            || line.contains("navi 4")
                            || line.contains("rdna 2")
                            || line.contains("rdna 3")
                            || line.contains("rdna 4")
                            || line.contains("rx 6")
                            || line.contains("rx 7")
                            || line.contains("rx 9")
                            || line.contains("van gogh")
                            || line.contains("rembrandt")
                            || line.contains("phoenix")
                            || line.contains("hawk point")
                            || line.contains("strix")
                            || line.contains("z1")
                            || line.contains("z2")
                        {
                            return true;
                        }
                    }
                }
            }
        }
    }

    // 2. Fallback: inspect /sys/class/drm/card*/device/uevent
    if let Ok(entries) = fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("card") && !name_str.contains('-') {
                let uevent_path = entry.path().join("device/uevent");
                if let Ok(content) = fs::read_to_string(uevent_path) {
                    let mut is_amd = false;
                    let mut pci_id = "";
                    for line in content.lines() {
                        if line == "DRIVER=amdgpu" {
                            is_amd = true;
                        }
                        if let Some(id) = line.strip_prefix("PCI_ID=") {
                            pci_id = id.trim();
                        }
                    }
                    if is_amd || pci_id.starts_with("1002:") {
                        let dev_id_hex = pci_id.split(':').nth(1).unwrap_or("");
                        if let Ok(dev_id) = u32::from_str_radix(dev_id_hex, 16) {
                            // Navi 2x, 3x, 4x: 0x7300..=0x75ff, modern APUs: 0x15bf..=0x16ff
                            if (0x7300..=0x75ff).contains(&dev_id)
                                || (0x15bf..=0x16ff).contains(&dev_id)
                            {
                                return true;
                            }
                        }
                    }
                }
            }
        }
    }

    false
}

fn is_executable_file(path: &Path) -> bool {
    if let Ok(meta) = fs::metadata(path) {
        meta.is_file() && (meta.permissions().mode() & 0o111 != 0)
    } else {
        false
    }
}

/// Verify that a Proton/Wine directory matches the host CPU architecture (`x86_64` vs `aarch64`),
/// checking both folder naming conventions and the ELF `e_machine` header of its Wine binary.
pub fn is_proton_dir_compatible_with_host(tool_dir: &Path) -> bool {
    let folder_lower = tool_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if cfg!(target_arch = "x86_64") {
        if folder_lower.contains("arm64") || folder_lower.contains("aarch64") {
            return false;
        }
    } else if cfg!(target_arch = "aarch64") {
        if folder_lower.contains("x86_64") || folder_lower.contains("amd64") {
            return false;
        }
    }

    let wine_candidates = [
        tool_dir.join("files/bin/wine64"),
        tool_dir.join("files/bin/wine"),
        tool_dir.join("dist/bin/wine64"),
        tool_dir.join("dist/bin/wine"),
        tool_dir.join("bin/wine64"),
        tool_dir.join("bin/wine"),
    ];

    for bin in &wine_candidates {
        if !bin.is_file() {
            continue;
        }
        let Ok(mut f) = fs::File::open(bin) else {
            continue;
        };
        let mut hdr = [0u8; 20];
        use std::io::Read;
        if f.read_exact(&mut hdr).is_ok() && &hdr[0..4] == b"\x7FELF" {
            let e_machine = u16::from_le_bytes([hdr[18], hdr[19]]);
            if cfg!(target_arch = "x86_64") {
                // EM_X86_64 (0x3E = 62) or EM_386 (0x03 = 3)
                if e_machine != 0x3E && e_machine != 0x03 {
                    return false;
                }
            } else if cfg!(target_arch = "aarch64") {
                // EM_AARCH64 (0xB7 = 183)
                if e_machine != 0xB7 {
                    return false;
                }
            }
            break;
        }
    }

    true
}

fn runner_priority_rank(name: &str) -> u8 {
    let lower = name.to_ascii_lowercase();
    if (lower.contains("ge-proton") || lower.contains("proton-ge"))
        && !lower.contains("rtsp")
        && !lower.contains("cachyos")
        && !lower.contains("dwproton")
    {
        0
    } else if lower.contains("dwproton") || lower.contains("dw-proton") {
        1
    } else if lower.contains("cachyos") {
        2
    } else if lower.contains("experimental") || lower.contains("hotfix") {
        3
    } else if lower.contains("rtsp") {
        5
    } else {
        4
    }
}

/// Discover all Proton & Wine runners on the system:
/// 1. Custom runners inside `compatibilitytools.d` (GE-Proton, Proton-CachyOS, etc.)
/// 2. Official Steam Proton builds (`Proton - Experimental`, `Proton Hotfix`, `Proton 9.0`, etc.)
///    inside `steamapps/common` across primary and secondary Steam libraries (`libraryfolders.vdf`).
pub fn discover_proton_tools() -> Vec<ProtonTool> {
    let home = real_user_home();
    let doors_compat_dir = crate::proton_manager::get_doors_compatibilitytools_dir();

    // (directory_path, require_proton_script_only, is_steam)
    let mut search_dirs: Vec<(PathBuf, bool, bool)> = vec![
        // Doors' own compatibilitytools.d
        (doors_compat_dir.clone(), false, false),
        (home.join(".local/share/doors/compatibilitytools.d"), false, false),

        // Steam compatibilitytools.d directories (native + Debian/Ubuntu + Flatpak Steam + host /usr/share/steam)
        (home.join(".local/share/Steam/compatibilitytools.d"), false, true),
        (home.join(".steam/root/compatibilitytools.d"), false, true),
        (home.join(".steam/steam/compatibilitytools.d"), false, true),
        (
            home.join(".steam/debian-installation/compatibilitytools.d"),
            false,
            true,
        ),
        (
            home.join(".var/app/com.valvesoftware.Steam/data/Steam/compatibilitytools.d"),
            false,
            true,
        ),
        (
            home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam/compatibilitytools.d"),
            false,
            true,
        ),
        (
            home.join(".var/app/com.valvesoftware.Steam/.steam/root/compatibilitytools.d"),
            false,
            true,
        ),
        (PathBuf::from("/usr/share/steam/compatibilitytools.d"), false, true),
        (
            PathBuf::from("/usr/local/share/steam/compatibilitytools.d"),
            false,
            true,
        ),
        (
            PathBuf::from("/run/host/usr/share/steam/compatibilitytools.d"),
            false,
            true,
        ),
        (
            PathBuf::from("/run/host/usr/local/share/steam/compatibilitytools.d"),
            false,
            true,
        ),
    ];

    if let Ok(extra) = std::env::var("DOORS_COMPAT_TOOLS_PATH") {
        for part in extra.split(':') {
            if !part.trim().is_empty() {
                search_dirs.push((PathBuf::from(part.trim()), false, false));
            }
        }
    }

    // Official Steam roots + libraryfolders.vdf discovery for Proton Hotfix / Experimental / 9.0 / etc.
    let steam_roots = [
        home.join(".local/share/Steam"),
        home.join(".steam/root"),
        home.join(".steam/steam"),
        home.join(".steam/debian-installation"),
        home.join(".var/app/com.valvesoftware.Steam/data/Steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        home.join(".var/app/com.valvesoftware.Steam/.steam/root"),
    ];

    for root in &steam_roots {
        search_dirs.push((root.join("steamapps/common"), true, true));
        let vdf_path = root.join("steamapps/libraryfolders.vdf");
        for lib_dir in parse_steam_library_folders(&vdf_path) {
            search_dirs.push((lib_dir.join("steamapps/common"), true, true));
            search_dirs.push((lib_dir.join("compatibilitytools.d"), false, true));
        }
    }

    // Also check direct SteamLibrary / Steam folders on mounted drives (/mnt/*, /var/mnt/*, /run/media/*/*, /media/*/*)
    let mut mounted_roots: Vec<PathBuf> = Vec::new();
    for mnt_base in ["/mnt", "/var/mnt"] {
        if let Ok(entries) = fs::read_dir(mnt_base) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    mounted_roots.push(p);
                }
            }
        }
    }
    for media_base in ["/run/media", "/media"] {
        if let Ok(users) = fs::read_dir(media_base) {
            for u in users.flatten() {
                if let Ok(mounts) = fs::read_dir(u.path()) {
                    for m in mounts.flatten() {
                        let p = m.path();
                        if p.is_dir() {
                            mounted_roots.push(p);
                        }
                    }
                }
            }
        }
    }
    for mp in mounted_roots {
        for sub in ["SteamLibrary", "Steam", "steam"] {
            let base = mp.join(sub);
            search_dirs.push((base.join("steamapps/common"), true, true));
            search_dirs.push((base.join("compatibilitytools.d"), false, true));
        }
        search_dirs.push((mp.join("steamapps/common"), true, true));
        search_dirs.push((mp.join("compatibilitytools.d"), false, true));
    }

    let mut tools = Vec::new();
    let mut seen_names = HashSet::new();
    let mut seen_canonical = HashSet::new();

    for (dir, steamapps_only_proton, is_steam) in search_dirs {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let tool_dir = entry.path();
            if !tool_dir.is_dir() {
                continue;
            }
            let canonical = fs::canonicalize(&tool_dir).unwrap_or_else(|_| tool_dir.clone());
            if seen_canonical.contains(&canonical) {
                continue;
            }

            let Some(folder_name) = tool_dir.file_name().and_then(|n| n.to_str()) else {
                continue;
            };

            if !is_proton_dir_compatible_with_host(&tool_dir) {
                continue;
            }

            let proton_script = tool_dir.join("proton");
            let files_wine = tool_dir.join("files/bin/wine");
            let dist_wine = tool_dir.join("dist/bin/wine");
            let bin_wine = tool_dir.join("bin/wine");

            let (executable, is_proton_script) = if proton_script.is_file() {
                (proton_script, true)
            } else if !steamapps_only_proton && files_wine.is_file() {
                (files_wine, false)
            } else if !steamapps_only_proton && dist_wine.is_file() {
                (dist_wine, false)
            } else if !steamapps_only_proton && bin_wine.is_file() {
                (bin_wine, false)
            } else {
                continue;
            };

            let mut display_name = if is_steam && !folder_name.ends_with("(Steam)") {
                format!("{} (Steam)", folder_name)
            } else {
                folder_name.to_string()
            };

            if seen_names.contains(&display_name) {
                display_name = format!("{} ({})", display_name, dir.display());
            }

            seen_canonical.insert(canonical);
            seen_names.insert(display_name.clone());

            tools.push(ProtonTool {
                name: display_name,
                dir_path: tool_dir,
                executable,
                is_proton_script,
            });
        }
    }

    // Sort: Doors' own Protons first, then Steam Protons;
    // within each group prefer general-purpose compatibility runners (GE-Proton > DW-Proton > Proton-CachyOS > others).
    tools.sort_by(|a, b| {
        let a_is_steam = a.name.ends_with("(Steam)");
        let b_is_steam = b.name.ends_with("(Steam)");
        match (a_is_steam, b_is_steam) {
            (false, true) => std::cmp::Ordering::Less,
            (true, false) => std::cmp::Ordering::Greater,
            _ => runner_priority_rank(&a.name)
                .cmp(&runner_priority_rank(&b.name))
                .then_with(|| b.name.to_ascii_lowercase().cmp(&a.name.to_ascii_lowercase())),
        }
    });
    tools
}

/// Parse `"path" "<steam_library_path>"` entries from Steam's `libraryfolders.vdf`.
fn parse_steam_library_folders(vdf_path: &Path) -> Vec<PathBuf> {
    let Ok(content) = fs::read_to_string(vdf_path) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("\"path\"") {
            let rest = rest.trim();
            if let Some(quoted) = rest.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                let unescaped = quoted.replace("\\\\", "\\");
                if !unescaped.is_empty() {
                    paths.push(PathBuf::from(unescaped));
                }
            }
        }
    }
    paths
}

/// Translate a sandbox `/run/host/...` path back to its real host root path `/...` when spawning on host.
fn to_host_path(path: &Path) -> PathBuf {
    if let Ok(rel) = path.strip_prefix("/run/host") {
        Path::new("/").join(rel)
    } else {
        path.to_path_buf()
    }
}

/// Find the Steam client install path for STEAM_COMPAT_CLIENT_INSTALL_PATH
pub fn detect_steam_install_path() -> PathBuf {
    let home = real_user_home();
    let candidates = [
        home.join(".local/share/Steam"),
        home.join(".steam/debian-installation"),
        home.join(".var/app/com.valvesoftware.Steam/data/Steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        home.join(".steam/root"),
        home.join(".steam/steam"),
    ];
    for c in candidates {
        if c.exists() {
            return c;
        }
    }
    home.join(".local/share/Steam")
}

/// Check if Steam is installed on the user's system.
pub fn is_steam_installed() -> bool {
    let home = real_user_home();
    let candidates = [
        home.join(".local/share/Steam"),
        home.join(".steam/debian-installation"),
        home.join(".var/app/com.valvesoftware.Steam/data/Steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        home.join(".var/app/com.valvesoftware.Steam/.steam/root"),
        home.join(".steam/root"),
        home.join(".steam/steam"),
    ];
    candidates.iter().any(|c| c.is_dir())
}

/// Find the Steam Linux Runtime `run` executable (e.g. SteamLinuxRuntime_sniper/run or soldier/run)
pub fn detect_steam_linux_runtime() -> Option<PathBuf> {
    let home = real_user_home();
    let steam_roots = [
        home.join(".local/share/Steam"),
        home.join(".steam/debian-installation"),
        home.join(".var/app/com.valvesoftware.Steam/data/Steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        home.join(".var/app/com.valvesoftware.Steam/.steam/root"),
        home.join(".steam/root"),
        home.join(".steam/steam"),
    ];

    let runtimes = [
        "SteamLinuxRuntime_sniper",
        "SteamLinuxRuntime_soldier",
        "SteamLinuxRuntime_medic",
        "SteamLinuxRuntime",
    ];

    let mut common_dirs = Vec::new();
    for root in &steam_roots {
        common_dirs.push(root.join("steamapps/common"));
        let vdf_path = root.join("steamapps/libraryfolders.vdf");
        for lib in parse_steam_library_folders(&vdf_path) {
            common_dirs.push(lib.join("steamapps/common"));
        }
    }

    if let Ok(entries) = fs::read_dir("/mnt") {
        for entry in entries.flatten() {
            common_dirs.push(entry.path().join("SteamLibrary/steamapps/common"));
            common_dirs.push(entry.path().join("steamapps/common"));
        }
    }

    for dir in common_dirs {
        for rt in &runtimes {
            let run_path = dir.join(rt).join("run");
            if run_path.is_file() {
                return Some(run_path);
            }
        }
    }

    None
}

/// Spawn a Windows `.exe` during initial setup before a `DoorsGameConfig` exists.
#[allow(dead_code)]
pub fn spawn_with_proton(
    tool: &ProtonTool,
    game_container_dir: &Path,
    exe_path: &Path,
) -> anyhow::Result<Child> {
    let default_cfg = DoorsGameConfig::new(
        "Setup".to_string(),
        "setup".to_string(),
        game_container_dir.to_path_buf(),
        exe_path.to_path_buf(),
        tool.name.clone(),
        tool.dir_path.clone(),
        None,
    );
    spawn_with_proton_config(tool, &default_cfg, Some(exe_path))
}

/// Spawn `winecfg` for this program's prefix.
pub fn spawn_winecfg(tool: &ProtonTool, cfg: &DoorsGameConfig) -> anyhow::Result<Child> {
    spawn_with_proton_config(tool, cfg, Some(Path::new("winecfg.exe")))
}

/// Spawn `winetricks` for this program's prefix.
pub fn spawn_winetricks(tool: &ProtonTool, cfg: &DoorsGameConfig) -> anyhow::Result<Child> {
    let pfx_dir = cfg.container_dir.join("pfx");
    fs::create_dir_all(&pfx_dir)?;

    let wine_bin = [
        tool.dir_path.join("files/bin/wine"),
        tool.dir_path.join("dist/bin/wine"),
        tool.dir_path.join("bin/wine"),
    ]
    .into_iter()
    .find(|p| p.is_file());

    let in_flatpak = Path::new("/.flatpak-info").exists();
    if in_flatpak {
        let mut cmd = Command::new("flatpak-spawn");
        cmd.arg("--host")
            .arg("--watch-bus")
            .arg(format!("--env=WINEPREFIX={}", pfx_dir.display()));
        if let Some(ref wb) = wine_bin {
            cmd.arg(format!("--env=WINE={}", to_host_path(wb).display()));
        }
        cmd.arg("winetricks").arg("--gui").stdin(Stdio::null());
        if let Ok(child) = cmd.spawn() {
            return Ok(child);
        }
    }

    let mut cmd = Command::new("winetricks");
    cmd.env("WINEPREFIX", &pfx_dir)
        .arg("--gui")
        .stdin(Stdio::null());
    if let Some(wb) = wine_bin {
        cmd.env("WINE", wb);
    }
    Ok(cmd.spawn()?)
}

/// Spawn a configured program using its `DoorsGameConfig` settings (SyncMode, Wayland, HDR,
/// Discrete GPU, NVAPI, FSR, vkBasalt, GameMode, MangoHud, Gamescope, and custom launch arguments).
pub fn spawn_with_proton_config(
    tool: &ProtonTool,
    cfg: &DoorsGameConfig,
    override_exe: Option<&Path>,
) -> anyhow::Result<Child> {
    let exe_path = override_exe.unwrap_or(&cfg.main_exe);
    let game_container_dir = &cfg.container_dir;
    let pfx_dir = game_container_dir.join("pfx");
    fs::create_dir_all(&pfx_dir)?;

    let steam_dir = detect_steam_install_path();
    let _ = fs::create_dir_all(&steam_dir);

    let work_dir = exe_path
        .parent()
        .filter(|p| p.exists() && !p.as_os_str().is_empty())
        .unwrap_or(game_container_dir);

    // Collect environment variables
    let mut envs: Vec<(String, String)> = vec![
        (
            "STEAM_COMPAT_DATA_PATH".to_string(),
            game_container_dir.display().to_string(),
        ),
        (
            "STEAM_COMPAT_CLIENT_INSTALL_PATH".to_string(),
            steam_dir.display().to_string(),
        ),
        ("WINEPREFIX".to_string(), pfx_dir.display().to_string()),
        ("SteamAppId".to_string(), "0".to_string()),
        ("SteamGameId".to_string(), "0".to_string()),
        (
            "WINEDLLOVERRIDES".to_string(),
            "winemenubuilder.exe=d".to_string(),
        ),
    ];

    // UMU / Protonfixes
    if cfg.umu && !cfg.umu_id.is_empty() {
        envs.push(("GAMEID".to_string(), cfg.umu_id.clone()));
        envs.push(("UMU_ID".to_string(), cfg.umu_id.clone()));
        if !cfg.umu_store.is_empty() && !cfg.umu_store.eq_ignore_ascii_case("none") {
            envs.push(("STORE".to_string(), cfg.umu_store.clone()));
        }
    } else {
        envs.push(("PROTONFIXES_DISABLE".to_string(), "1".to_string()));
    }

    // Synchronization mode
    match cfg.sync_mode {
        SyncMode::Ntsync => {
            envs.push(("PROTON_USE_NTSYNC".to_string(), "1".to_string()));
        }
        SyncMode::Fsync => {
            envs.push(("PROTON_NO_NTSYNC".to_string(), "1".to_string()));
            envs.push(("WINEFSYNC".to_string(), "1".to_string()));
        }
        SyncMode::Esync => {
            envs.push(("PROTON_NO_NTSYNC".to_string(), "1".to_string()));
            envs.push(("PROTON_NO_FSYNC".to_string(), "1".to_string()));
            envs.push(("WINEESYNC".to_string(), "1".to_string()));
        }
        SyncMode::None => {
            envs.push(("PROTON_NO_NTSYNC".to_string(), "1".to_string()));
            envs.push(("PROTON_NO_FSYNC".to_string(), "1".to_string()));
            envs.push(("PROTON_NO_ESYNC".to_string(), "1".to_string()));
        }
    }

    // Native Wayland
    if cfg.wayland {
        envs.push(("PROTON_ENABLE_WAYLAND".to_string(), "1".to_string()));
    }

    // High Dynamic Range (HDR)
    if cfg.hdr {
        envs.push(("PROTON_ENABLE_HDR".to_string(), "1".to_string()));
        envs.push(("DXVK_HDR".to_string(), "1".to_string()));
        envs.push(("ENABLE_HDR_WSI".to_string(), "1".to_string()));
    }

    // Discrete GPU (DRI_PRIME + NVIDIA Prime Offload)
    if cfg.discrete_gpu {
        envs.push(("DRI_PRIME".to_string(), "1".to_string()));
        envs.push(("__NV_PRIME_RENDER_OFFLOAD".to_string(), "1".to_string()));
        envs.push((
            "__GLX_VENDOR_LIBRARY_NAME".to_string(),
            "nvidia".to_string(),
        ));
        envs.push((
            "__VK_LAYER_NV_optimus".to_string(),
            "NVIDIA_only".to_string(),
        ));
    }

    // NVIDIA DLSS / NVAPI
    if cfg.nvapi {
        envs.push(("PROTON_ENABLE_NVAPI".to_string(), "1".to_string()));
        envs.push(("PROTON_DLSS_UPGRADE".to_string(), "1".to_string()));
    }

    // AMD FSR (driver-level spatial upscaling)
    if cfg.fsr {
        envs.push(("WINE_FULLSCREEN_FSR".to_string(), "1".to_string()));
        envs.push((
            "WINE_FULLSCREEN_FSR_STRENGTH".to_string(),
            cfg.fsr_strength.min(5).to_string(),
        ));
        if !cfg.fsr_mode.trim().is_empty() {
            envs.push((
                "WINE_FULLSCREEN_FSR_MODE".to_string(),
                cfg.fsr_mode.trim().to_lowercase(),
            ));
        }
    }

    // AMD FSR 4 DLL upgrade (works independently for native FSR 3.1 games)
    if cfg.fsr4 {
        envs.push(("PROTON_FSR4_UPGRADE".to_string(), "1".to_string()));
    }

    // Ray Tracing (DXR)
    if cfg.ray_tracing {
        envs.push(("VKD3D_CONFIG".to_string(), "dxr11,dxr".to_string()));
        envs.push(("PROTON_ENABLE_RAYTRACING".to_string(), "1".to_string()));
    }

    // DXVK Async (dxvk-gplasync)
    if cfg.dxvk_async {
        envs.push(("DXVK_ASYNC".to_string(), "1".to_string()));
    }

    // vkBasalt
    if cfg.vkbasalt && has_vkbasalt() {
        envs.push(("ENABLE_VKBASALT".to_string(), "1".to_string()));
    }

    // MangoHud
    if cfg.mangohud && has_host_binary("mangohud") {
        envs.push(("MANGOHUD".to_string(), "1".to_string()));
        envs.push(("MANGOHUD_DLSYM".to_string(), "1".to_string()));
    }

    // DXVK HUD (built-in FPS and GPU load counter)
    if cfg.dxvk_hud {
        envs.push(("DXVK_HUD".to_string(), "fps,gpuload".to_string()));
    }

    // Parse custom launch_args: support `KEY=VAL`, wrappers before `%command%`, and exe args
    let (extra_envs, custom_prefix_args, exe_args) = parse_launch_options(&cfg.launch_args);
    envs.extend(extra_envs);

    // Build full command line vector
    let mut cmd_parts: Vec<String> = Vec::new();

    if cfg.gamescope && has_host_binary("gamescope") {
        cmd_parts.push("gamescope".to_string());
        if cfg.gamescope_width > 0 && cfg.gamescope_height > 0 {
            cmd_parts.push("-W".to_string());
            cmd_parts.push(cfg.gamescope_width.to_string());
            cmd_parts.push("-H".to_string());
            cmd_parts.push(cfg.gamescope_height.to_string());
        }
        if cfg.gamescope_fps > 0 {
            cmd_parts.push("-r".to_string());
            cmd_parts.push(cfg.gamescope_fps.to_string());
        }
        if cfg.gamescope_fullscreen {
            cmd_parts.push("-f".to_string());
        }
        if cfg.hdr {
            cmd_parts.push("--hdr-enabled".to_string());
        }
        cmd_parts.push("--".to_string());
    }

    if cfg.gamemode && has_host_binary("gamemoderun") {
        cmd_parts.push("gamemoderun".to_string());
    }

    let in_flatpak = Path::new("/.flatpak-info").exists();
    let host_tool_dir = if in_flatpak {
        to_host_path(&tool.dir_path)
    } else {
        tool.dir_path.clone()
    };
    let host_tool_exec = if in_flatpak {
        to_host_path(&tool.executable)
    } else {
        tool.executable.clone()
    };

    if cfg.steam_runtime {
        if let Some(slr_run) = detect_steam_linux_runtime() {
            let slr_exec = if in_flatpak {
                to_host_path(&slr_run)
            } else {
                slr_run
            };
            let mut mount_paths = vec![
                game_container_dir.display().to_string(),
                host_tool_dir.display().to_string(),
                work_dir.display().to_string(),
            ];
            for extra in ["/mnt", "/var/mnt", "/run/media", "/media"] {
                if Path::new(extra).is_dir() {
                    mount_paths.push(extra.to_string());
                }
            }
            mount_paths.dedup();
            envs.push(("STEAM_COMPAT_MOUNTS".to_string(), mount_paths.join(":")));
            cmd_parts.push(slr_exec.display().to_string());
            cmd_parts.push("--".to_string());
        }
    }

    cmd_parts.extend(custom_prefix_args);

    cmd_parts.push(host_tool_exec.display().to_string());
    if tool.is_proton_script {
        cmd_parts.push("waitforexitandrun".to_string());
    }
    let is_msi = exe_path
        .extension()
        .and_then(|e| e.to_str())
        .map_or(false, |e| e.eq_ignore_ascii_case("msi"));
    if is_msi {
        cmd_parts.push("msiexec.exe".to_string());
        cmd_parts.push("/i".to_string());
    }
    cmd_parts.push(exe_path.display().to_string());
    cmd_parts.extend(exe_args);

    if in_flatpak {
        let mut cmd = Command::new("flatpak-spawn");
        cmd.arg("--host")
            .arg("--watch-bus")
            .arg(format!("--directory={}", work_dir.display()));

        for (k, v) in &envs {
            cmd.arg(format!("--env={}={}", k, v));
        }
        for part in &cmd_parts {
            cmd.arg(part);
        }
        cmd.stdin(Stdio::null());
        if let Ok(child) = cmd.spawn() {
            return Ok(child);
        }
    }

    let (first_prog, rest_args) = cmd_parts
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("Empty launch command"))?;

    let mut cmd = Command::new(first_prog);
    cmd.current_dir(work_dir).stdin(Stdio::null());
    for (k, v) in &envs {
        cmd.env(k, v);
    }
    for arg in rest_args {
        cmd.arg(arg);
    }

    let child = cmd.spawn()?;
    Ok(child)
}

/// Wait until all Wine child processes inside `pfx_dir` (such as detached Stage-2 setup wizards)
/// have exited and flushed disk/registry writes.
pub fn wait_for_prefix_wineserver(tool: &ProtonTool, pfx_dir: &Path) {
    let candidates = [
        tool.dir_path.join("files/bin/wineserver"),
        tool.dir_path.join("dist/bin/wineserver"),
        tool.dir_path.join("bin/wineserver"),
    ];
    let Some(ws_bin) = candidates.into_iter().find(|p| p.is_file()) else {
        return;
    };

    let in_flatpak = Path::new("/.flatpak-info").exists();
    if in_flatpak {
        let _ = Command::new("flatpak-spawn")
            .arg("--host")
            .arg(format!("--env=WINEPREFIX={}", pfx_dir.display()))
            .arg(to_host_path(&ws_bin))
            .arg("-w")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    } else {
        let _ = Command::new(&ws_bin)
            .env("WINEPREFIX", pfx_dir)
            .arg("-w")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn parse_launch_options(raw: &str) -> (Vec<(String, String)>, Vec<String>, Vec<String>) {
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    let mut envs = Vec::new();
    let mut prefix_cmd = Vec::new();
    let mut exe_args = Vec::new();

    if tokens.is_empty() {
        return (envs, prefix_cmd, exe_args);
    }

    if let Some(cmd_idx) = tokens.iter().position(|&t| t == "%command%") {
        let before = &tokens[..cmd_idx];
        let after = &tokens[cmd_idx + 1..];
        let mut env_phase = true;
        for &tok in before {
            if env_phase {
                if let Some((k, v)) = tok.split_once('=') {
                    if !k.is_empty() && !k.starts_with('-') {
                        envs.push((k.to_string(), v.to_string()));
                        continue;
                    }
                }
                env_phase = false;
            }
            prefix_cmd.push(tok.to_string());
        }
        for &tok in after {
            exe_args.push(tok.to_string());
        }
    } else {
        let mut env_phase = true;
        for &tok in &tokens {
            if env_phase {
                if let Some((k, v)) = tok.split_once('=') {
                    if !k.is_empty() && !k.starts_with('-') {
                        envs.push((k.to_string(), v.to_string()));
                        continue;
                    }
                }
                env_phase = false;
            }
            exe_args.push(tok.to_string());
        }
    }

    (envs, prefix_cmd, exe_args)
}

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

fn is_executable_file(path: &Path) -> bool {
    if let Ok(meta) = fs::metadata(path) {
        meta.is_file() && (meta.permissions().mode() & 0o111 != 0)
    } else {
        false
    }
}

/// Discover all Proton & Wine runners on the system:
/// 1. Custom runners inside `compatibilitytools.d` (GE-Proton, Proton-CachyOS, etc.)
/// 2. Official Steam Proton builds (`Proton - Experimental`, `Proton Hotfix`, `Proton 9.0`, etc.)
///    inside `steamapps/common` across primary and secondary Steam libraries (`libraryfolders.vdf`).
pub fn discover_proton_tools() -> Vec<ProtonTool> {
    let home = real_user_home();

    // (directory_path, require_proton_script_only)
    let mut search_dirs: Vec<(PathBuf, bool)> = vec![
        (home.join(".local/share/Steam/compatibilitytools.d"), false),
        (home.join(".steam/root/compatibilitytools.d"), false),
        (home.join(".steam/steam/compatibilitytools.d"), false),
        (
            home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam/compatibilitytools.d"),
            false,
        ),
        (
            home.join(".var/app/com.valvesoftware.Steam/.steam/root/compatibilitytools.d"),
            false,
        ),
        (PathBuf::from("/usr/share/steam/compatibilitytools.d"), false),
        (
            PathBuf::from("/usr/local/share/steam/compatibilitytools.d"),
            false,
        ),
    ];

    if let Ok(extra) = std::env::var("DOORS_COMPAT_TOOLS_PATH") {
        for part in extra.split(':') {
            if !part.trim().is_empty() {
                search_dirs.push((PathBuf::from(part.trim()), false));
            }
        }
    }

    // Official Steam roots + libraryfolders.vdf discovery for Proton Hotfix / Experimental / 9.0 / etc.
    let steam_roots = [
        home.join(".local/share/Steam"),
        home.join(".steam/root"),
        home.join(".steam/steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        home.join(".var/app/com.valvesoftware.Steam/.steam/root"),
    ];

    for root in &steam_roots {
        search_dirs.push((root.join("steamapps/common"), true));
        let vdf_path = root.join("steamapps/libraryfolders.vdf");
        for lib_dir in parse_steam_library_folders(&vdf_path) {
            search_dirs.push((lib_dir.join("steamapps/common"), true));
            search_dirs.push((lib_dir.join("compatibilitytools.d"), false));
        }
    }

    // Also check direct SteamLibrary folders on mounted drives (/mnt/*, /run/media/*/*)
    if let Ok(entries) = fs::read_dir("/mnt") {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                search_dirs.push((p.join("SteamLibrary/steamapps/common"), true));
            }
        }
    }
    for media_base in ["/run/media", "/media"] {
        if let Ok(users) = fs::read_dir(media_base) {
            for u in users.flatten() {
                if let Ok(mounts) = fs::read_dir(u.path()) {
                    for m in mounts.flatten() {
                        search_dirs.push((m.path().join("SteamLibrary/steamapps/common"), true));
                    }
                }
            }
        }
    }

    let mut tools = Vec::new();
    let mut seen_names = HashSet::new();
    let mut seen_canonical = HashSet::new();

    for (dir, steamapps_only_proton) in search_dirs {
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

            let mut display_name = folder_name.to_string();
            if seen_names.contains(&display_name) {
                display_name = format!("{} ({})", folder_name, dir.display());
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

    tools.sort_by(|a, b| b.name.cmp(&a.name));
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

/// Find the Steam client install path for STEAM_COMPAT_CLIENT_INSTALL_PATH
pub fn detect_steam_install_path() -> PathBuf {
    let home = real_user_home();
    let candidates = [
        home.join(".local/share/Steam"),
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

/// Spawn a Windows `.exe` during initial setup before a `DoorsGameConfig` exists.
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
            cmd.arg(format!("--env=WINE={}", wb.display()));
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

    // AMD FSR
    if cfg.fsr {
        envs.push(("WINE_FULLSCREEN_FSR".to_string(), "1".to_string()));
        envs.push(("PROTON_FSR4_UPGRADE".to_string(), "1".to_string()));
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

    cmd_parts.extend(custom_prefix_args);

    cmd_parts.push(tool.executable.display().to_string());
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

    let in_flatpak = Path::new("/.flatpak-info").exists();

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

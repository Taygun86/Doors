use crate::disks::{real_user_home, DiskInfo};
use crate::icon::pe_has_icon;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    #[default]
    Fsync,
    Ntsync,
    Esync,
    None,
}

fn default_gamemode() -> bool {
    crate::proton::has_host_binary("gamemoderun")
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoorsGameConfig {
    pub name: String,
    pub slug: String,
    pub container_dir: PathBuf,
    pub main_exe: PathBuf,
    pub proton_name: String,
    pub proton_dir: PathBuf,
    pub icon_png: Option<PathBuf>,

    #[serde(default)]
    pub launch_args: String,
    #[serde(default)]
    pub sync_mode: SyncMode,
    #[serde(default)]
    pub wayland: bool,
    #[serde(default)]
    pub hdr: bool,
    #[serde(default = "default_gamemode")]
    pub gamemode: bool,
    #[serde(default)]
    pub mangohud: bool,
    #[serde(default)]
    pub gamescope: bool,
    #[serde(default)]
    pub gamescope_width: u32,
    #[serde(default)]
    pub gamescope_height: u32,
    #[serde(default)]
    pub gamescope_fps: u32,
    #[serde(default = "default_true")]
    pub gamescope_fullscreen: bool,
    #[serde(default = "default_true")]
    pub discrete_gpu: bool,
    #[serde(default)]
    pub nvapi: bool,
    #[serde(default)]
    pub fsr: bool,
    #[serde(default)]
    pub vkbasalt: bool,
}

impl DoorsGameConfig {
    pub fn new(
        name: String,
        slug: String,
        container_dir: PathBuf,
        main_exe: PathBuf,
        proton_name: String,
        proton_dir: PathBuf,
        icon_png: Option<PathBuf>,
    ) -> Self {
        let sync_mode = if Path::new("/dev/ntsync").exists() {
            SyncMode::Ntsync
        } else {
            SyncMode::Fsync
        };
        Self {
            name,
            slug,
            container_dir,
            main_exe,
            proton_name,
            proton_dir,
            icon_png,
            launch_args: String::new(),
            sync_mode,
            wayland: false,
            hdr: false,
            gamemode: default_gamemode(),
            mangohud: false,
            gamescope: false,
            gamescope_width: 0,
            gamescope_height: 0,
            gamescope_fps: 0,
            gamescope_fullscreen: true,
            discrete_gpu: true,
            nvapi: false,
            fsr: false,
            vkbasalt: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum InstallMode {
    /// Portable / Pre-installed program directory (e.g. SteamRIP) to be moved to target disk
    PortableFolder {
        source_dir: PathBuf,
        clicked_rel_exe: PathBuf,
    },
    /// Standalone or repack installer (`setup.exe`, `*install*.exe`, etc.) to be executed inside target prefix
    SetupInstaller { installer_exe: PathBuf },
    /// Single standalone `.exe` in a protected directory like `~/Downloads` (may be an installer or single-file app)
    SingleExe { exe_path: PathBuf },
}

#[derive(Debug, Clone)]
pub struct InstallPlan {
    pub game_name: String,
    pub slug: String,
    pub mode: InstallMode,
}

/// If `cfg.main_exe` is missing or points to a setup/installer binary while a real installed `.exe`
/// exists inside `pfx/drive_c` or `game/`, heal `cfg` and persist `doors.json` + `.desktop`.
fn heal_config_if_needed(cfg: &mut DoorsGameConfig) {
    if is_auxiliary_exe(&cfg.main_exe) || !cfg.main_exe.is_file() {
        if let Some((better_exe, _)) = detect_main_executable(&cfg.container_dir, None, &cfg.name) {
            if better_exe != cfg.main_exe {
                cfg.main_exe = better_exe;
                let icon_png_path = cfg.container_dir.join("icon.png");
                if crate::icon::extract_exe_icon_to_png(
                    &cfg.main_exe,
                    cfg.main_exe.parent(),
                    &icon_png_path,
                ) {
                    cfg.icon_png = Some(icon_png_path);
                }
                let _ = crate::desktop::save_config_and_create_desktop_entry(cfg);
            }
        }
    }
}

/// Check if `exe_path` is already inside an installed Doors container (`doors.json`).
pub fn find_existing_doors_config(exe_path: &Path) -> Option<(PathBuf, DoorsGameConfig)> {
    let canonical = fs::canonicalize(exe_path).unwrap_or_else(|_| exe_path.to_path_buf());
    let mut current = canonical.parent();
    while let Some(dir) = current {
        let cfg_path = dir.join("doors.json");
        if cfg_path.is_file() {
            if let Ok(content) = fs::read_to_string(&cfg_path) {
                if let Ok(mut cfg) = serde_json::from_str::<DoorsGameConfig>(&content) {
                    cfg.container_dir = dir.to_path_buf();
                    heal_config_if_needed(&mut cfg);
                    return Some((cfg_path, cfg));
                }
            }
        }
        current = dir.parent();
    }
    None
}

/// Scan all discovered disks for `<mount>/Doors/*/doors.json`.
pub fn discover_installed_games(disks: &[DiskInfo]) -> Vec<DoorsGameConfig> {
    let mut games = Vec::new();
    for disk in disks {
        let Ok(entries) = fs::read_dir(&disk.doors_root) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let cfg_path = dir.join("doors.json");
            if cfg_path.is_file() {
                if let Ok(data) = fs::read_to_string(&cfg_path) {
                    if let Ok(mut cfg) = serde_json::from_str::<DoorsGameConfig>(&data) {
                        cfg.container_dir = dir;
                        heal_config_if_needed(&mut cfg);
                        games.push(cfg);
                    }
                }
            }
        }
    }
    games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    games
}

impl InstallPlan {
    pub fn analyze(exe_path: &Path) -> Self {
        let exe_abs = fs::canonicalize(exe_path).unwrap_or_else(|_| exe_path.to_path_buf());
        let file_stem = exe_abs
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Program");
        let file_name_lower = exe_abs
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        let parent_dir = exe_abs.parent().unwrap_or_else(|| Path::new("/tmp"));

        // 1. Check if this is an installer (`*setup*`, `*install*`, `autorun.exe`, FitGirl/GOG repack)
        let is_installer_name = file_name_lower.contains("setup")
            || file_name_lower.contains("install")
            || file_name_lower == "autorun.exe"
            || file_name_lower.contains("repack");

        let has_installer_bins = parent_dir.join("fg-01.bin").exists()
            || parent_dir.join("setup-1.bin").exists()
            || parent_dir.join("data-1.bin").exists()
            || parent_dir.join("data1.cab").exists();

        if is_installer_name || has_installer_bins {
            let raw_title = if file_name_lower == "setup.exe"
                || file_name_lower == "install.exe"
                || file_name_lower == "installer.exe"
                || file_name_lower == "autorun.exe"
            {
                if !is_protected_directory(parent_dir) {
                    parent_dir
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(file_stem)
                } else {
                    file_stem
                }
            } else {
                file_stem
            };
            let game_name = clean_game_title(raw_title);
            let slug = slugify(&game_name);
            return InstallPlan {
                game_name,
                slug,
                mode: InstallMode::SetupInstaller {
                    installer_exe: exe_abs,
                },
            };
        }

        // 2. If parent directory is a protected user folder (e.g. ~/Downloads directly),
        // do NOT move the entire ~/Downloads folder!
        if is_protected_directory(parent_dir) {
            let game_name = clean_game_title(file_stem);
            let slug = slugify(&game_name);
            return InstallPlan {
                game_name,
                slug,
                mode: InstallMode::SingleExe { exe_path: exe_abs },
            };
        }

        // 3. Otherwise, it's inside a dedicated folder (like SteamRIP / portable folder).
        let source_dir = find_portable_root_folder(&exe_abs);
        let clicked_rel_exe = exe_abs
            .strip_prefix(&source_dir)
            .unwrap_or_else(|_| Path::new(&file_name_lower))
            .to_path_buf();

        let folder_title = source_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(file_stem);
        let game_name = clean_game_title(folder_title);
        let slug = slugify(&game_name);

        InstallPlan {
            game_name,
            slug,
            mode: InstallMode::PortableFolder {
                source_dir,
                clicked_rel_exe,
            },
        }
    }
}

fn find_portable_root_folder(exe_abs: &Path) -> PathBuf {
    let mut current = exe_abs.parent().unwrap_or_else(|| Path::new("/tmp"));

    let sub_folder_names = [
        "bin",
        "x64",
        "x86",
        "win64",
        "win32",
        "binaries",
        "shipping",
        "release",
        "retail",
        "game",
        "engine",
        "build",
    ];

    loop {
        let Some(parent) = current.parent() else {
            break;
        };
        if is_protected_directory(parent) {
            break;
        }
        let dir_name = current
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_lowercase();

        if sub_folder_names.contains(&dir_name.as_str()) {
            current = parent;
            continue;
        }

        if parent.join("_CommonRedist").is_dir() || parent.join("Engine").is_dir() {
            current = parent;
            continue;
        }

        break;
    }

    current.to_path_buf()
}

/// Returns true if `dir` is a system or top-level user folder that must NEVER be moved as a program folder.
pub fn is_protected_directory(dir: &Path) -> bool {
    let home = real_user_home();
    let canonical = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());

    if canonical == Path::new("/")
        || canonical == Path::new("/home")
        || canonical == Path::new("/mnt")
        || canonical == Path::new("/run")
        || canonical == Path::new("/run/media")
        || canonical == Path::new("/media")
        || canonical == Path::new("/tmp")
        || canonical == home
    {
        return true;
    }

    if canonical.parent() == Some(home.as_path()) {
        return true;
    }

    if canonical.parent() == Some(Path::new("/mnt")) {
        return true;
    }
    if let Some(p1) = canonical.parent() {
        if p1.parent() == Some(Path::new("/run/media")) || p1.parent() == Some(Path::new("/media"))
        {
            return true;
        }
    }

    false
}

/// Strip scene/release/installer suffixes like `-SteamRIP`, `_install_ua_ef67ba8459b7`, `setup_`, etc.
pub fn clean_game_title(raw: &str) -> String {
    let mut s = raw.to_string();

    while let (Some(start), Some(end)) = (s.find('['), s.find(']')) {
        if end > start {
            s.replace_range(start..=end, " ");
        } else {
            break;
        }
    }

    // If there is `_install` or `-install` followed by region/hash tags (e.g. `GenshinImpact_install_ua_ef67ba8459b7`),
    // truncate from `_install` / `-install` if there is a meaningful prefix before it!
    for sep_pat in ["_install", "-install", "_setup", "-setup"] {
        let lower = s.to_lowercase();
        if let Some(idx) = lower.find(sep_pat) {
            if idx >= 3 {
                s.truncate(idx);
            }
        }
    }

    let junk_patterns = [
        "-SteamRIP.com",
        "SteamRIP.com",
        "-SteamRIP",
        "_SteamRIP",
        "SteamRIP",
        "-SteamGG",
        "-FitGirl",
        "FitGirl Repack",
        "setup_",
        "_setup",
        "-setup",
        "installer",
        "install_",
    ];
    for pat in junk_patterns {
        let lower = s.to_lowercase();
        let pat_lower = pat.to_lowercase();
        if let Some(idx) = lower.find(&pat_lower) {
            s.replace_range(idx..idx + pat.len(), " ");
        }
    }

    let cleaned = s
        .replace('_', " ")
        .replace('.', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    let trimmed = cleaned.trim_matches(|c: char| c == '-' || c == ' ').trim();
    if trimmed.is_empty() {
        raw.to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    let res = out.trim_end_matches('-').to_string();
    if res.is_empty() {
        "doors-app".to_string()
    } else {
        res
    }
}

/// Returns true if an `.exe` filename is an installer, redistributable, crash handler, or uninstaller
/// rather than an installed main program executable.
pub fn is_auxiliary_exe(exe_path: &Path) -> bool {
    let fname = exe_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let path_lower = exe_path.to_string_lossy().to_lowercase();
    if path_lower.contains("_commonredist")
        || path_lower.contains("/redist/")
        || path_lower.contains("/directx/")
    {
        return true;
    }

    let excluded_substrings = [
        "unins",
        "uninstall",
        "setup",
        "install",
        "autorun",
        "unitycrashhandler",
        "crashreport",
        "crashpad",
        "crs-handler",
        "vc_redist",
        "vcredist",
        "dxsetup",
        "dxwebsetup",
        "dotnet",
        "ndp4",
        "ue4prereq",
        "ueprereq",
        "oalinst",
        "physx",
        "senddebug",
        "bugreport",
        "activationui",
        "cleanup",
        "touchup",
        "zfgamebrowser",
        "elevate",
        "7z.exe",
        "7za.exe",
    ];

    for pat in excluded_substrings {
        if fname.contains(pat) {
            return true;
        }
    }

    false
}

/// Detect the main executable (`.exe`) and optional refined program title after installation or folder move.
pub fn detect_main_executable(
    container_dir: &Path,
    preferred_exe: Option<&Path>,
    game_name_hint: &str,
) -> Option<(PathBuf, Option<String>)> {
    // 1. If a preferred_exe was provided (e.g. Portable/SteamRIP clicked .exe) and it is NOT an installer/auxiliary exe,
    // verify it exists and return it immediately!
    if let Some(pref) = preferred_exe {
        if pref.is_file() && !is_auxiliary_exe(pref) {
            return Some((pref.to_path_buf(), None));
        }
    }

    let pfx_drive_c = container_dir.join("pfx/drive_c");

    // 2. Check Windows Desktop and Start Menu `.lnk` shortcuts inside `pfx/drive_c`
    if pfx_drive_c.is_dir() {
        let mut shortcut_dirs = vec![
            pfx_drive_c.join("ProgramData/Microsoft/Windows/Start Menu/Programs"),
        ];
        if let Ok(users) = fs::read_dir(pfx_drive_c.join("users")) {
            for user_entry in users.flatten() {
                let u_path = user_entry.path();
                if u_path.is_dir() {
                    shortcut_dirs.push(u_path.join("Desktop"));
                    shortcut_dirs.push(
                        u_path.join("AppData/Roaming/Microsoft/Windows/Start Menu/Programs"),
                    );
                }
            }
        }

        let mut lnk_candidates: Vec<(i64, PathBuf, String)> = Vec::new();

        for sdir in shortcut_dirs {
            if !sdir.is_dir() {
                continue;
            }
            for entry in WalkDir::new(&sdir).max_depth(4).into_iter().flatten() {
                let p = entry.path();
                if p.extension()
                    .and_then(|e| e.to_str())
                    .map_or(false, |e| e.eq_ignore_ascii_case("lnk"))
                {
                    let lnk_stem = p
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string();
                    let lnk_lower = lnk_stem.to_lowercase();
                    if lnk_lower.contains("uninstall") || lnk_lower.contains("unins") {
                        continue;
                    }

                    if let Some(target_exe) = resolve_lnk_to_linux_path(p, &pfx_drive_c) {
                        if target_exe.is_file() && !is_auxiliary_exe(&target_exe) {
                            let is_desktop = sdir.ends_with("Desktop");
                            let prio = if is_desktop { 1000 } else { 500 };
                            lnk_candidates.push((prio, target_exe, clean_game_title(&lnk_stem)));
                        }
                    }
                }
            }
        }

        lnk_candidates.sort_by(|a, b| b.0.cmp(&a.0));
        if let Some((_, target_exe, lnk_title)) = lnk_candidates.into_iter().next() {
            return Some((target_exe, Some(lnk_title)));
        }
    }

    // 3. Scan `game/` directory and `pfx/drive_c/` for `.exe` candidates and score them
    let mut scan_roots = Vec::new();
    let game_dir = container_dir.join("game");
    if game_dir.is_dir() {
        scan_roots.push(game_dir);
    }
    if pfx_drive_c.is_dir() {
        scan_roots.push(pfx_drive_c.clone());
    }

    let mut best_exe: Option<(i64, PathBuf)> = None;
    let hint_lower = game_name_hint.to_lowercase();

    for root in scan_roots {
        for entry in WalkDir::new(&root)
            .max_depth(6)
            .into_iter()
            .filter_entry(|e| !is_ignored_windows_system_dir(e.path(), &pfx_drive_c))
            .flatten()
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if !ext.eq_ignore_ascii_case("exe") {
                continue;
            }
            if is_auxiliary_exe(path) {
                continue;
            }

            let mut score: i64 = 0;
            let stem_lower = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();

            // Prefer executables with an embedded icon
            if pe_has_icon(path) {
                score += 500;
            }

            // Prefer executables matching the program name hint or named `launcher`
            if !hint_lower.is_empty()
                && (stem_lower.contains(&hint_lower) || hint_lower.contains(&stem_lower))
            {
                score += 450;
            } else if stem_lower == "launcher" || stem_lower.ends_with("launcher") {
                score += 300;
            }

            // Prefer shallower depth relative to root
            let depth = entry.depth() as i64;
            score += (6 - depth).max(0) * 60;

            // Factor in file size (real binaries are usually > 200 KB)
            if let Ok(meta) = path.metadata() {
                let kb = meta.len() / 1024;
                if kb > 500 {
                    score += 200;
                }
                if kb > 5000 {
                    score += 150;
                }
            }

            if best_exe.as_ref().map_or(true, |(b_score, _)| score > *b_score) {
                best_exe = Some((score, path.to_path_buf()));
            }
        }
    }

    best_exe.map(|(_, path)| {
        // Also try deriving a clean title from the parent folder in Program Files if generic
        let folder_title = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .filter(|&n| {
                !n.eq_ignore_ascii_case("bin")
                    && !n.eq_ignore_ascii_case("x64")
                    && !n.eq_ignore_ascii_case("win64")
                    && !n.eq_ignore_ascii_case("game")
            })
            .map(clean_game_title);
        (path, folder_title)
    })
}

fn is_ignored_windows_system_dir(path: &Path, drive_c: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(drive_c) else {
        return false;
    };
    let rel_lower = rel.to_string_lossy().to_lowercase();
    rel_lower == "windows"
        || rel_lower.starts_with("windows/")
        || rel_lower == "program files/common files"
        || rel_lower == "program files (x86)/common files"
        || rel_lower == "program files/internet explorer"
        || rel_lower == "program files (x86)/internet explorer"
        || rel_lower == "program files/windows nt"
        || rel_lower == "program files (x86)/windows nt"
        || rel_lower == "program files/windows media player"
        || rel_lower == "program files (x86)/windows media player"
        || rel_lower == "program files/windows mail"
        || rel_lower == "program files (x86)/windows mail"
        || rel_lower == "program files/steam"
        || rel_lower.starts_with("program files/steam/")
        || rel_lower == "program files (x86)/steam"
        || rel_lower.starts_with("program files (x86)/steam/")
        || rel_lower == "openxr"
        || rel_lower.starts_with("openxr/")
        || rel_lower == "vrclient"
        || rel_lower.starts_with("vrclient/")
        || rel_lower.contains("/appdata/local/microsoft")
        || rel_lower.contains("/appdata/local/temp")
}

/// Extract target `.exe` path from a Windows Shell Link (`.lnk`) binary file
/// and resolve `C:\...` to `pfx/drive_c/...`.
fn resolve_lnk_to_linux_path(lnk_path: &Path, drive_c: &Path) -> Option<PathBuf> {
    let bytes = fs::read(lnk_path).ok()?;
    if bytes.len() < 0x4C || &bytes[0..4] != b"\x4C\x00\x00\x00" {
        return None;
    }

    let mut full_path_candidates = Vec::new();
    let mut exe_name_candidates = Vec::new();

    // Extract ASCII strings
    for s in extract_ascii_strings(&bytes) {
        let lower = s.to_lowercase();
        if (lower.starts_with("c:\\") || lower.starts_with("z:\\")) && lower.ends_with(".exe") {
            full_path_candidates.push(s);
        } else if lower.ends_with(".exe") && !lower.contains('\\') && !lower.contains('/') {
            exe_name_candidates.push(s);
        }
    }

    // Extract UTF-16LE strings
    for s in extract_utf16le_strings(&bytes) {
        let lower = s.to_lowercase();
        if (lower.starts_with("c:\\") || lower.starts_with("z:\\")) && lower.ends_with(".exe") {
            full_path_candidates.push(s);
        } else if lower.ends_with(".exe") && !lower.contains('\\') && !lower.contains('/') {
            exe_name_candidates.push(s);
        }
    }

    // 1. Try full paths (`C:\Program Files\...\.exe`)
    for win_path in full_path_candidates {
        let normalized = win_path.replace('\\', "/");
        let resolved = if normalized.starts_with("C:/") || normalized.starts_with("c:/") {
            drive_c.join(&normalized[3..])
        } else if normalized.starts_with("Z:/") || normalized.starts_with("z:/") {
            PathBuf::from(format!("/{}", &normalized[3..]))
        } else {
            continue;
        };

        if let Some(existing) = resolve_case_insensitive_path(&resolved) {
            if existing.is_file() && !is_auxiliary_exe(&existing) {
                return Some(existing);
            }
        }
    }

    // 2. Fallback: PIDL item `.exe` filename match inside `drive_c`
    for exe_name in exe_name_candidates {
        let exe_lower = exe_name.to_lowercase();
        for entry in WalkDir::new(drive_c)
            .max_depth(6)
            .into_iter()
            .filter_entry(|e| !is_ignored_windows_system_dir(e.path(), drive_c))
            .flatten()
        {
            let p = entry.path();
            if p.is_file() {
                if let Some(fname) = p.file_name().and_then(|n| n.to_str()) {
                    if fname.to_lowercase() == exe_lower && !is_auxiliary_exe(p) {
                        return Some(p.to_path_buf());
                    }
                }
            }
        }
    }

    None
}

fn extract_ascii_strings(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = Vec::new();
    for &b in bytes {
        if (0x20..0x7F).contains(&b) {
            current.push(b);
        } else {
            if current.len() >= 5 {
                if let Ok(s) = String::from_utf8(current.clone()) {
                    out.push(s);
                }
            }
            current.clear();
        }
    }
    out
}

fn extract_utf16le_strings(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    for offset in [0usize, 1usize] {
        let mut current = Vec::new();
        let mut i = offset;
        while i + 1 < bytes.len() {
            let u = u16::from_le_bytes([bytes[i], bytes[i + 1]]);
            if (0x20..0x7F).contains(&u) || (0xA0..0xFFFD).contains(&u) {
                current.push(u);
            } else {
                if current.len() >= 5 {
                    if let Ok(s) = String::from_utf16(&current) {
                        out.push(s);
                    }
                }
                current.clear();
            }
            i += 2;
        }
    }
    out
}

fn resolve_case_insensitive_path(target: &Path) -> Option<PathBuf> {
    if target.exists() {
        return Some(target.to_path_buf());
    }
    let mut current = PathBuf::from("/");
    for comp in target.components() {
        use std::path::Component;
        match comp {
            Component::RootDir => current = PathBuf::from("/"),
            Component::Normal(part) => {
                let candidate = current.join(part);
                if candidate.exists() {
                    current = candidate;
                } else {
                    let part_lower = part.to_string_lossy().to_lowercase();
                    let entries = fs::read_dir(&current).ok()?;
                    let mut matched = None;
                    for entry in entries.flatten() {
                        if entry.file_name().to_string_lossy().to_lowercase() == part_lower {
                            matched = Some(entry.path());
                            break;
                        }
                    }
                    current = matched?;
                }
            }
            _ => {}
        }
    }
    if current.exists() {
        Some(current)
    } else {
        None
    }
}

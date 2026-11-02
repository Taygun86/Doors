use crate::disks::{real_user_home, DiskInfo};
use crate::icon::{
    extract_exe_metadata, icon_source_dimensions, is_generic_metadata_boilerplate, pe_has_icon,
    ExeMetadata,
};
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

fn default_fsr_strength() -> u32 {
    2
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
    #[serde(default = "default_fsr_strength")]
    pub fsr_strength: u32,
    #[serde(default)]
    pub fsr_mode: String,
    #[serde(default)]
    pub fsr4: bool,
    #[serde(default)]
    pub ray_tracing: bool,
    #[serde(default)]
    pub dxvk_async: bool,
    #[serde(default)]
    pub dxvk_hud: bool,
    #[serde(default)]
    pub vkbasalt: bool,
    #[serde(default)]
    pub desktop_shortcut: bool,
    #[serde(default)]
    pub steam_runtime: bool,
    #[serde(default)]
    pub umu: bool,
    #[serde(default)]
    pub umu_id: String,
    #[serde(default)]
    pub umu_store: String,
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
            fsr_strength: 2,
            fsr_mode: String::new(),
            fsr4: false,
            ray_tracing: false,
            dxvk_async: false,
            dxvk_hud: false,
            vkbasalt: false,
            desktop_shortcut: false,
            steam_runtime: false,
            umu: false,
            umu_id: String::new(),
            umu_store: String::new(),
        }
    }

    /// Renames the container folder on disk, updates all internal paths (`container_dir`,
    /// `main_exe`, `icon_png`), and handles collisions safely.
    pub fn rename_container(&mut self, new_name: &str) -> anyhow::Result<()> {
        let trimmed = new_name.trim();
        if trimmed.is_empty()
            || trimmed.contains('/')
            || trimmed.contains('\\')
            || trimmed.contains('\0')
            || trimmed == "."
            || trimmed == ".."
        {
            anyhow::bail!("Invalid container folder name");
        }

        let current_name = self
            .container_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        if current_name == trimmed {
            return Ok(());
        }

        let Some(parent) = self.container_dir.parent().map(|p| p.to_path_buf()) else {
            anyhow::bail!("No parent directory for container");
        };
        let target_dir = parent.join(trimmed);

        if target_dir.exists() {
            let is_same = match (
                fs::canonicalize(&self.container_dir),
                fs::canonicalize(&target_dir),
            ) {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            };
            if !is_same {
                anyhow::bail!("Target directory already exists");
            }
        }

        let old_dir = self.container_dir.clone();
        fs::rename(&old_dir, &target_dir)?;

        self.container_dir = target_dir.clone();
        if let Ok(rel) = self.main_exe.strip_prefix(&old_dir) {
            self.main_exe = target_dir.join(rel);
        }
        if let Some(ref icon) = self.icon_png {
            if let Ok(rel) = icon.strip_prefix(&old_dir) {
                self.icon_png = Some(target_dir.join(rel));
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum InstallMode {
    /// Portable / Pre-installed program directory to be moved to target disk
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
    pub umu_match: Option<crate::umu::UmuEntry>,
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

                        // If desktop shortcut is missing (e.g. after system reinstall/format), automatically restore it
                        let home = real_user_home();
                        let desk = home
                            .join(".local/share/applications")
                            .join(format!("doors-{}.desktop", cfg.slug));
                        if !desk.exists() {
                            let _ = crate::desktop::create_desktop_shortcut(&cfg);
                        }

                        if cfg.desktop_shortcut && !crate::desktop::has_desktop_folder_shortcut(&cfg) {
                            let _ = crate::desktop::create_desktop_folder_shortcut(&cfg);
                        }

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
        let exe_file_name = exe_abs
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let file_name_lower = exe_file_name.to_lowercase();

        let parent_dir = exe_abs.parent().unwrap_or_else(|| Path::new("/tmp"));
        let exe_meta = extract_exe_metadata(&exe_abs);

        // 1. Check if this is an installer (`*.msi`, `*setup*`, `*install*`, `autorun.exe`, or multipart archive)
        let is_installer_name = file_name_lower.ends_with(".msi")
            || file_name_lower.contains("setup")
            || file_name_lower.contains("install")
            || file_name_lower == "autorun.exe";

        let has_installer_bins = parent_dir.join("setup-1.bin").exists()
            || parent_dir.join("data-1.bin").exists()
            || parent_dir.join("data1.cab").exists();

        if is_installer_name || has_installer_bins || exe_meta.is_installer_binary() {
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
            let local_title = choose_local_title(raw_title, &exe_meta, true);
            let umu_match = resolve_plan_umu_match(&local_title, exe_file_name, &exe_meta, None);
            let game_name = refine_title_with_umu(local_title, raw_title, &exe_meta, umu_match.as_ref(), false);
            let slug = slugify(&game_name);
            return InstallPlan {
                game_name,
                slug,
                mode: InstallMode::SetupInstaller {
                    installer_exe: exe_abs,
                },
                umu_match,
            };
        }

        // 2. If parent directory is a protected user folder (e.g. ~/Downloads directly),
        // do NOT move the entire ~/Downloads folder!
        if is_protected_directory(parent_dir) {
            let local_title = choose_local_title(file_stem, &exe_meta, true);
            let umu_match = resolve_plan_umu_match(&local_title, exe_file_name, &exe_meta, None);
            let game_name = refine_title_with_umu(local_title, file_stem, &exe_meta, umu_match.as_ref(), false);
            let slug = slugify(&game_name);
            return InstallPlan {
                game_name,
                slug,
                mode: InstallMode::SingleExe { exe_path: exe_abs },
                umu_match,
            };
        }

        // 3. Otherwise, it's inside a dedicated folder (portable game directory).
        let source_dir = find_portable_root_folder(&exe_abs);
        let clicked_rel_exe = exe_abs
            .strip_prefix(&source_dir)
            .unwrap_or_else(|_| Path::new(&file_name_lower))
            .to_path_buf();

        let folder_title = source_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(file_stem);
        let local_title = choose_local_title(folder_title, &exe_meta, false);
        let umu_match =
            resolve_plan_umu_match(&local_title, exe_file_name, &exe_meta, Some(&source_dir));
        let game_name =
            refine_title_with_umu(local_title, folder_title, &exe_meta, umu_match.as_ref(), true);
        let slug = slugify(&game_name);

        InstallPlan {
            game_name,
            slug,
            mode: InstallMode::PortableFolder {
                source_dir,
                clicked_rel_exe,
            },
            umu_match,
        }
    }
}

fn choose_local_title(raw_name: &str, exe_meta: &ExeMetadata, is_standalone_exe: bool) -> String {
    let cleaned_raw = clean_game_title(raw_name);
    let Some(meta_title) = exe_meta.best_title() else {
        return cleaned_raw;
    };

    if is_generic_metadata_boilerplate(&cleaned_raw) {
        return meta_title;
    }

    if is_standalone_exe {
        // Keep `cleaned_raw` only if `meta_title` is a shorter launcher prefix of `cleaned_raw`
        // (e.g. meta_title = "StudioLauncher", cleaned_raw = "StudioLauncher subproject")
        let raw_lower = cleaned_raw.to_lowercase();
        let meta_lower = meta_title.to_lowercase();
        if raw_lower.starts_with(&meta_lower) && raw_lower.len() > meta_lower.len() + 2 {
            return cleaned_raw;
        }
        return meta_title;
    }

    cleaned_raw
}

fn resolve_plan_umu_match(
    local_title: &str,
    exe_file_name: &str,
    exe_meta: &ExeMetadata,
    portable_dir: Option<&Path>,
) -> Option<crate::umu::UmuEntry> {
    let mut signals = exe_meta.candidate_signals();
    let push_sig = |vec: &mut Vec<String>, s: &str| {
        let t = s.trim();
        if !t.is_empty() && !vec.iter().any(|x| x.eq_ignore_ascii_case(t)) {
            vec.push(t.to_string());
        }
    };

    push_sig(&mut signals, local_title);
    push_sig(&mut signals, exe_file_name);

    // If inside a portable folder and the clicked exe had no embedded metadata (e.g. thin launcher),
    // also sample up to 3 sibling main .exe files for embedded PE metadata.
    if exe_meta.best_title().is_none() {
        if let Some(dir) = portable_dir {
            if !is_protected_directory(dir) {
                let mut checked = 0usize;
                for entry in WalkDir::new(dir).max_depth(2).into_iter().flatten() {
                    if checked >= 3 {
                        break;
                    }
                    let p = entry.path();
                    if !p.is_file() {
                        continue;
                    }
                    if p.extension()
                        .and_then(|e| e.to_str())
                        .map_or(false, |e| e.eq_ignore_ascii_case("exe"))
                        && !is_auxiliary_exe(p)
                    {
                        checked += 1;
                        for sig in extract_exe_metadata(p).candidate_signals() {
                            push_sig(&mut signals, &sig);
                        }
                    }
                }
            }
        }
    }

    let sig_refs: Vec<&str> = signals.iter().map(|s| s.as_str()).collect();
    crate::umu::lookup_umu_with_signals(&sig_refs)
}

fn refine_title_with_umu(
    local_title: String,
    raw_source: &str,
    exe_meta: &ExeMetadata,
    umu_match: Option<&crate::umu::UmuEntry>,
    is_portable_folder: bool,
) -> String {
    let Some(matched) = umu_match else {
        return local_title;
    };
    let clean_umu = crate::umu::clean_umu_display_title(&matched.title);
    if clean_umu.is_empty() {
        return local_title;
    }

    if is_generic_metadata_boilerplate(&local_title) {
        return clean_umu;
    }

    let local_lower = local_title.to_lowercase();
    let umu_lower = clean_umu.to_lowercase();

    // If local title already starts with the clean UMU title (e.g. "Game Name LC"), preserve the user's edition label
    if local_lower.starts_with(&umu_lower) {
        return local_title;
    }

    if !is_portable_folder {
        // For standalone .exe / setup installers, if the filename had version/underscore noise or was a single-token slug,
        // and PE metadata did not already give a matching full title, upgrade to the clean UMU title.
        let had_filename_noise =
            raw_source.contains('_') || raw_source.contains('.') || !raw_source.contains(' ');
        let meta_matches_umu = exe_meta
            .best_title()
            .map(|m| m.to_lowercase().starts_with(&umu_lower))
            .unwrap_or(false);
        if had_filename_noise && !meta_matches_umu {
            return clean_umu;
        }
    }

    local_title
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

/// Strip version tags, bracketed metadata, architecture markers, and installer suffixes.
pub fn clean_game_title(raw: &str) -> String {
    let mut s = raw.to_string();

    while let (Some(start), Some(end)) = (s.find('['), s.find(']')) {
        if end > start {
            s.replace_range(start..=end, " ");
        } else {
            break;
        }
    }

    while let (Some(start), Some(end)) = (s.find('('), s.find(')')) {
        if end > start {
            s.replace_range(start..=end, " ");
        } else {
            break;
        }
    }

    // Truncate installer/setup suffix tags if there is a meaningful title before it
    for sep_pat in ["_install", "-install", "_setup", "-setup"] {
        let lower = s.to_ascii_lowercase();
        if let Some(idx) = lower.find(sep_pat) {
            if idx >= 3 {
                s.truncate(idx);
            }
        }
    }

    let junk_patterns = [
        "setup_",
        "_setup",
        "-setup",
        "_installer",
        "-installer",
        "installer",
        "install_",
        "-portable",
        "_portable",
        "portable",
        "standalone",
    ];
    for pat in junk_patterns {
        loop {
            let lower = s.to_ascii_lowercase();
            let Some(idx) = lower.find(pat) else {
                break;
            };
            s.replace_range(idx..idx + pat.len(), " ");
        }
    }

    // Turn kebab-case hyphen-separated words (e.g. "My-Adventure-Game") into normal spaces
    if s.matches('-').count() >= 2 {
        s = s.replace('-', " ");
    }

    // Strip generic internet domain patterns (e.g. words ending in .com, .org, .net, etc.)
    let domain_tlds = [
        ".com", ".net", ".org", ".io", ".gg", ".tv", ".ru", ".to", ".cc", ".me", ".biz",
    ];
    for tld in domain_tlds {
        loop {
            let lower = s.to_ascii_lowercase();
            let Some(idx) = lower.find(tld) else {
                break;
            };
            let prefix = &s[..idx];
            let start = prefix.rfind([' ', '_']).map(|i| i + 1).unwrap_or(0);
            s.replace_range(start..idx + tld.len(), " ");
        }
    }

    // Replace underscores with spaces, and replace non-version dots with spaces (preserving digit.digit in version tokens)
    let s_underscores = s.replace('_', " ");
    let chars: Vec<char> = s_underscores.chars().collect();
    let mut dot_preserved = String::with_capacity(s_underscores.len());
    for i in 0..chars.len() {
        if chars[i] == '.' {
            let prev_digit = i > 0 && chars[i - 1].is_ascii_digit();
            let next_digit = i + 1 < chars.len() && chars[i + 1].is_ascii_digit();
            if prev_digit && next_digit {
                dot_preserved.push('.');
            } else {
                dot_preserved.push(' ');
            }
        } else {
            dot_preserved.push(chars[i]);
        }
    }

    let mut filtered_words: Vec<&str> = Vec::new();
    let mut in_version_tail = false;

    for tok in dot_preserved.split_whitespace() {
        let tok_clean = tok.trim_matches('-');
        if tok_clean.is_empty() {
            continue;
        }
        if is_version_token(tok_clean) {
            in_version_tail = true;
            continue;
        }
        if in_version_tail && tok_clean.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        in_version_tail = false;

        if is_arch_or_build_token(tok_clean) {
            continue;
        }
        filtered_words.push(tok);
    }

    if let Some(last) = filtered_words.last() {
        let tlds = ["com", "net", "org", "ru", "to", "io", "cc", "gg", "tv", "me"];
        if tlds.iter().any(|t| last.eq_ignore_ascii_case(t)) {
            filtered_words.pop();
        }
    }

    let trimmed = filtered_words
        .join(" ")
        .replace('.', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| c == '-' || c == ' ')
        .trim()
        .to_string();

    if trimmed.is_empty() {
        raw.to_string()
    } else {
        trimmed
    }
}

fn is_version_token(tok: &str) -> bool {
    let lower = tok.to_ascii_lowercase();
    let v_rest = lower
        .strip_prefix("version")
        .or_else(|| lower.strip_prefix("ver"))
        .or_else(|| lower.strip_prefix('v'));

    if let Some(rest) = v_rest {
        let rest = rest.trim_start_matches(['.', '-']);
        if let Some(first) = rest.chars().next() {
            if first.is_ascii_digit()
                && rest
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c.is_ascii_lowercase())
            {
                return true;
            }
        }
    }

    if lower.contains('.') {
        let parts: Vec<&str> = lower.split('.').collect();
        if parts.len() >= 2
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        {
            return true;
        }
    }

    false
}

fn is_arch_or_build_token(tok: &str) -> bool {
    let lower = tok.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "x64" | "x86" | "x86_64" | "amd64" | "win64" | "win32" | "64bit" | "32bit"
    ) {
        return true;
    }

    for prefix in ["build", "bld", "rev"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let rest = rest.trim_start_matches(['-', '.']);
            if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                return true;
            }
        }
    }

    false
}

/// Sanitize a display title into a filesystem- and Wine/NTFS-safe container folder name.
pub fn sanitize_folder_name(name: &str) -> String {
    let replaced = name.replace(": ", " - ").replace(':', "-");
    let clean: String = replaced
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | '*' | '?' | '"' | '<' | '>' | '|') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let collapsed = clean
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| c == '.' || c == ' ')
        .to_string();
    if collapsed.is_empty() || collapsed == "." || collapsed == ".." {
        "DoorsApp".to_string()
    } else {
        collapsed
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

    if fname.ends_with(".msi") {
        return true;
    }

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
    // 1. If a preferred_exe was provided (e.g. clicked portable .exe) and it is NOT an installer/auxiliary exe,
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

    // 2b. Check Wine Registry `Uninstall` keys (`pfx/system.reg` & `pfx/user.reg`)
    // Catches installers that don't create Desktop/Start Menu shortcuts or install onto a custom `Z:\` path.
    let (reg_exe_candidates, reg_install_dirs) = scan_registry_uninstall_entries(container_dir);
    if let Some((reg_exe, reg_title)) = reg_exe_candidates.into_iter().next() {
        return Some((reg_exe, reg_title));
    }

    // 3. Scan `game/` directory, registry `InstallLocation` folders, and `pfx/drive_c/` for `.exe` candidates
    let mut scan_roots = Vec::new();
    let game_dir = container_dir.join("game");
    if game_dir.is_dir() {
        scan_roots.push(game_dir);
    }
    for reg_dir in reg_install_dirs {
        if reg_dir.is_dir() && !is_protected_directory(&reg_dir) && !scan_roots.contains(&reg_dir) {
            scan_roots.push(reg_dir);
        }
    }
    if pfx_drive_c.is_dir() {
        scan_roots.push(pfx_drive_c.clone());
    }

    let mut best_exe: Option<(i64, PathBuf)> = None;
    let hint_lower = game_name_hint.to_lowercase();

    for root in scan_roots {
        for entry in WalkDir::new(&root)
            .max_depth(10)
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
            score += (8 - depth).max(0) * 50;

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
        let meta_title = extract_exe_metadata(&path).best_title();
        // Prefer embedded PE ProductName/FileDescription; fall back to parent folder name in Program Files
        let discovered_title = meta_title.or_else(|| {
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .filter(|&n| {
                    !n.eq_ignore_ascii_case("bin")
                        && !n.eq_ignore_ascii_case("x64")
                        && !n.eq_ignore_ascii_case("win64")
                        && !n.eq_ignore_ascii_case("game")
                        && !n.eq_ignore_ascii_case("programs")
                        && !n.eq_ignore_ascii_case("program files")
                        && !n.eq_ignore_ascii_case("program files (x86)")
                })
                .map(clean_game_title)
        });
        (path, discovered_title)
    })
}

fn is_ignored_windows_system_dir(path: &Path, drive_c: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(drive_c) else {
        return false;
    };
    let rel_lower = rel.to_string_lossy().to_lowercase();
    rel_lower == "windows"
        || rel_lower.starts_with("windows/")
        || rel_lower == "programdata/microsoft"
        || rel_lower.starts_with("programdata/microsoft/")
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

/// Inspect Wine's `pfx/system.reg` and `pfx/user.reg` `Uninstall` entries to discover installed executables
/// and custom `InstallLocation` directories (including `Z:\...` host paths).
fn scan_registry_uninstall_entries(
    container_dir: &Path,
) -> (Vec<(PathBuf, Option<String>)>, Vec<PathBuf>) {
    let pfx_dir = container_dir.join("pfx");
    let drive_c = pfx_dir.join("drive_c");
    let mut exe_hits = Vec::new();
    let mut dir_hits = Vec::new();

    for reg_name in ["user.reg", "system.reg"] {
        let reg_path = pfx_dir.join(reg_name);
        let Ok(content) = fs::read_to_string(&reg_path) else {
            continue;
        };

        let mut in_uninstall = false;
        let mut display_name: Option<String> = None;
        let mut display_icon: Option<String> = None;
        let mut install_location: Option<String> = None;

        let mut flush_section = |d_name: &mut Option<String>,
                                 d_icon: &mut Option<String>,
                                 i_loc: &mut Option<String>| {
            let name_opt = d_name.take();
            let icon_opt = d_icon.take();
            let loc_opt = i_loc.take();

            if let Some(ref n) = name_opt {
                let nl = n.to_lowercase();
                if nl.starts_with("microsoft visual c++")
                    || nl.starts_with("microsoft .net")
                    || nl.starts_with("wine ")
                    || nl.contains("redistributable")
                {
                    return;
                }
            }

            let clean_title = name_opt
                .as_deref()
                .map(clean_game_title)
                .filter(|t| !t.is_empty() && !is_generic_metadata_boilerplate(t));

            if let Some(icon_raw) = icon_opt {
                if let Some(resolved_exe) = resolve_windows_path_to_linux(&icon_raw, &drive_c) {
                    let is_exe = resolved_exe
                        .extension()
                        .and_then(|e| e.to_str())
                        .map_or(false, |e| e.eq_ignore_ascii_case("exe"));
                    if is_exe && resolved_exe.is_file() && !is_auxiliary_exe(&resolved_exe) {
                        exe_hits.push((resolved_exe, clean_title.clone()));
                    }
                }
            }

            if let Some(loc_raw) = loc_opt {
                if let Some(resolved_dir) = resolve_windows_path_to_linux(&loc_raw, &drive_c) {
                    if resolved_dir.is_dir() && !dir_hits.contains(&resolved_dir) {
                        dir_hits.push(resolved_dir);
                    }
                }
            }
        };

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                if in_uninstall {
                    flush_section(&mut display_name, &mut display_icon, &mut install_location);
                }
                let lower = trimmed.to_ascii_lowercase();
                in_uninstall =
                    lower.contains("currentversion\\\\uninstall\\\\") && !lower.ends_with("uninstall]");
                continue;
            }

            if !in_uninstall {
                continue;
            }

            if let Some((k, v)) = parse_wine_reg_key_value(trimmed) {
                if k.eq_ignore_ascii_case("DisplayName") {
                    display_name = Some(v);
                } else if k.eq_ignore_ascii_case("DisplayIcon") {
                    display_icon = Some(v);
                } else if k.eq_ignore_ascii_case("InstallLocation") && !v.trim().is_empty() {
                    install_location = Some(v);
                }
            }
        }

        if in_uninstall {
            flush_section(&mut display_name, &mut display_icon, &mut install_location);
        }
    }

    (exe_hits, dir_hits)
}

fn parse_wine_reg_key_value(line: &str) -> Option<(String, String)> {
    if !line.starts_with('"') {
        return None;
    }
    let rest = &line[1..];
    let key_end = rest.find('"')?;
    let key = rest[..key_end].to_string();
    let after_key = rest[key_end + 1..].trim_start();
    let val_raw = after_key.strip_prefix('=')?.trim();
    let val_inner = val_raw.strip_prefix('"')?.strip_suffix('"')?;

    let mut unescaped = String::with_capacity(val_inner.len());
    let mut chars = val_inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek().copied() {
                Some('\\') => {
                    chars.next();
                    unescaped.push('\\');
                }
                Some('"') => {
                    chars.next();
                    unescaped.push('"');
                }
                _ => unescaped.push('\\'),
            }
        } else {
            unescaped.push(c);
        }
    }
    Some((key, unescaped))
}

fn strip_win_path_decorations(raw: &str) -> String {
    let mut s = raw.trim().trim_matches('"').trim().to_string();
    // Strip trailing icon index like `,0` or `,-101`
    if let Some(comma_idx) = s.rfind(',') {
        let suffix = s[comma_idx + 1..].trim();
        if !suffix.is_empty()
            && suffix
                .chars()
                .all(|c| c.is_ascii_digit() || c == '-')
        {
            s.truncate(comma_idx);
            s = s.trim().trim_matches('"').trim().to_string();
        }
    }
    // If arguments follow `.exe" ...` or `.exe ...` when quote-wrapped, trim at `.exe`
    let lower = s.to_ascii_lowercase();
    if let Some(exe_pos) = lower.find(".exe") {
        let after = &s[exe_pos + 4..];
        if after.starts_with('"') || after.starts_with(' ') {
            s.truncate(exe_pos + 4);
        }
    }
    s.trim_matches('"').trim().to_string()
}

fn expand_windows_env_vars(win_path: &str, drive_c: &Path) -> Vec<String> {
    if !win_path.contains('%') {
        return vec![win_path.to_string()];
    }

    let mut user_names = vec!["steamuser".to_string()];
    if let Ok(entries) = fs::read_dir(drive_c.join("users")) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.eq_ignore_ascii_case("Public") && !user_names.contains(&name) {
                    user_names.push(name);
                }
            }
        }
    }

    let mut results = Vec::new();
    for user in &user_names {
        let mut expanded = win_path.to_string();
        let replacements = [
            ("%ProgramFiles(x86)%", "C:\\Program Files (x86)"),
            ("%ProgramFiles%", "C:\\Program Files"),
            ("%ProgramW6432%", "C:\\Program Files"),
            ("%CommonProgramFiles%", "C:\\Program Files\\Common Files"),
            ("%ProgramData%", "C:\\ProgramData"),
            ("%PUBLIC%", "C:\\users\\Public"),
            ("%SystemDrive%", "C:"),
            (
                "%LOCALAPPDATA%",
                &format!("C:\\users\\{user}\\AppData\\Local"),
            ),
            (
                "%APPDATA%",
                &format!("C:\\users\\{user}\\AppData\\Roaming"),
            ),
            ("%USERPROFILE%", &format!("C:\\users\\{user}")),
        ];

        for (var, val) in replacements {
            let lower_check = expanded.to_ascii_lowercase();
            let var_lower = var.to_ascii_lowercase();
            if let Some(idx) = lower_check.find(&var_lower) {
                expanded.replace_range(idx..idx + var.len(), val);
            }
        }
        if !results.contains(&expanded) {
            results.push(expanded);
        }
    }
    results
}

fn resolve_windows_path_to_linux(raw_win_path: &str, drive_c: &Path) -> Option<PathBuf> {
    let cleaned = strip_win_path_decorations(raw_win_path);
    if cleaned.is_empty() {
        return None;
    }

    for candidate in expand_windows_env_vars(&cleaned, drive_c) {
        let normalized = candidate.replace('\\', "/");
        let resolved = if normalized.starts_with("C:/") || normalized.starts_with("c:/") {
            drive_c.join(&normalized[3..])
        } else if normalized.starts_with("Z:/") || normalized.starts_with("z:/") {
            PathBuf::from(format!("/{}", &normalized[3..]))
        } else {
            continue;
        };

        if let Some(existing) = resolve_case_insensitive_path(&resolved) {
            return Some(existing);
        }
    }
    None
}

/// Extract target `.exe` path from a Windows Shell Link (`.lnk`) binary file
/// and resolve `C:\...` or `Z:\...` to its Linux path.
pub(crate) fn resolve_lnk_to_linux_path(lnk_path: &Path, drive_c: &Path) -> Option<PathBuf> {
    let bytes = fs::read(lnk_path).ok()?;
    if bytes.len() < 0x4C || &bytes[0..4] != b"\x4C\x00\x00\x00" {
        return None;
    }

    let mut full_path_candidates = parse_shllink_structured_paths(&bytes);
    let mut exe_name_candidates = Vec::new();

    // Substring fallback over ASCII & UTF-16LE runs (immune to CountedString length-prefix bytes)
    for s in extract_ascii_strings(&bytes)
        .into_iter()
        .chain(extract_utf16le_strings(&bytes))
    {
        for sub in extract_win_exe_substrings(&s) {
            if !full_path_candidates.contains(&sub) {
                full_path_candidates.push(sub);
            }
        }
        let trimmed = s.trim();
        if trimmed.to_ascii_lowercase().ends_with(".exe")
            && !trimmed.contains('\\')
            && !trimmed.contains('/')
            && trimmed.len() <= 128
        {
            exe_name_candidates.push(trimmed.to_string());
        }
    }

    // 1. Try full paths (`C:\Program Files\...\.exe`, `Z:\...\.exe`, `%LOCALAPPDATA%\...\.exe`)
    for win_path in full_path_candidates {
        if let Some(existing) = resolve_windows_path_to_linux(&win_path, drive_c) {
            if existing.is_file() && !is_auxiliary_exe(&existing) {
                return Some(existing);
            }
        }
    }

    // 2. Fallback: PIDL item `.exe` filename match inside `drive_c`
    for exe_name in exe_name_candidates {
        let exe_lower = exe_name.to_lowercase();
        for entry in WalkDir::new(drive_c)
            .max_depth(10)
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

/// Parse the official MS-SHLLINK binary header (`LinkInfo`, `StringData` `CountedString`s, and
/// `EnvironmentVariableDataBlock`) without corrupting paths whose length falls in `0x20..0x7F`.
fn parse_shllink_structured_paths(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let read_u16 = |off: usize| -> Option<u16> {
        let s = bytes.get(off..off + 2)?;
        Some(u16::from_le_bytes([s[0], s[1]]))
    };
    let read_u32 = |off: usize| -> Option<u32> {
        let s = bytes.get(off..off + 4)?;
        Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    };
    let read_cstr_ansi = |off: usize, limit: usize| -> Option<String> {
        if off >= limit || limit > bytes.len() {
            return None;
        }
        let slice = &bytes[off..limit];
        let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
        let s = String::from_utf8_lossy(&slice[..end]).trim().to_string();
        (!s.is_empty()).then_some(s)
    };
    let read_cstr_utf16 = |off: usize, limit: usize| -> Option<String> {
        if off + 2 > limit || limit > bytes.len() {
            return None;
        }
        let mut u16s = Vec::new();
        let mut p = off;
        while p + 2 <= limit {
            let ch = u16::from_le_bytes([bytes[p], bytes[p + 1]]);
            if ch == 0 {
                break;
            }
            u16s.push(ch);
            p += 2;
        }
        let s = String::from_utf16_lossy(&u16s).trim().to_string();
        (!s.is_empty()).then_some(s)
    };

    let Some(link_flags) = read_u32(0x14) else {
        return out;
    };
    let mut pos: usize = 0x4C;

    // 1. LinkTargetIDList (HasLinkTargetIDList = 0x01)
    if link_flags & 0x01 != 0 {
        let Some(id_list_size) = read_u16(pos) else {
            return out;
        };
        pos = pos.saturating_add(2 + id_list_size as usize);
    }

    // 2. LinkInfo (HasLinkInfo = 0x02)
    if link_flags & 0x02 != 0 {
        if let Some(info_size) = read_u32(pos) {
            let info_end = pos.saturating_add(info_size as usize);
            if info_size >= 0x1C && info_end <= bytes.len() {
                let hdr_size = read_u32(pos + 4).unwrap_or(0);
                let info_flags = read_u32(pos + 8).unwrap_or(0);
                // VolumeIDAndLocalBasePath = 0x01
                if info_flags & 0x01 != 0 {
                    if hdr_size >= 0x24 {
                        let base_u = read_u32(pos + 0x1C).unwrap_or(0) as usize;
                        let suf_u = read_u32(pos + 0x20).unwrap_or(0) as usize;
                        if base_u > 0 {
                            let b = read_cstr_utf16(pos + base_u, info_end).unwrap_or_default();
                            let sf = if suf_u > 0 {
                                read_cstr_utf16(pos + suf_u, info_end).unwrap_or_default()
                            } else {
                                String::new()
                            };
                            let combined = join_win_base_and_suffix(&b, &sf);
                            if !combined.is_empty() {
                                out.push(combined);
                            }
                        }
                    }
                    let base_a = read_u32(pos + 0x10).unwrap_or(0) as usize;
                    let suf_a = read_u32(pos + 0x18).unwrap_or(0) as usize;
                    if base_a > 0 {
                        let b = read_cstr_ansi(pos + base_a, info_end).unwrap_or_default();
                        let sf = if suf_a > 0 {
                            read_cstr_ansi(pos + suf_a, info_end).unwrap_or_default()
                        } else {
                            String::new()
                        };
                        let combined = join_win_base_and_suffix(&b, &sf);
                        if !combined.is_empty() && !out.contains(&combined) {
                            out.push(combined);
                        }
                    }
                }
            }
            pos = info_end;
        }
    }

    // 3. StringData CountedStrings (Name=0x04, RelativePath=0x08, WorkingDir=0x10, Arguments=0x20, IconLocation=0x40)
    let is_unicode = (link_flags & 0x80) != 0;
    let mut rel_path: Option<String> = None;
    let mut work_dir: Option<String> = None;
    let mut icon_loc: Option<String> = None;

    for flag in [0x04u32, 0x08, 0x10, 0x20, 0x40] {
        if link_flags & flag == 0 {
            continue;
        }
        let Some(char_count) = read_u16(pos) else {
            break;
        };
        pos += 2;
        let byte_len = if is_unicode {
            (char_count as usize) * 2
        } else {
            char_count as usize
        };
        let Some(slice) = bytes.get(pos..pos + byte_len) else {
            break;
        };
        pos += byte_len;

        let val = if is_unicode {
            let u16s: Vec<u16> = slice
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&u16s).trim().to_string()
        } else {
            String::from_utf8_lossy(slice).trim().to_string()
        };

        if val.is_empty() {
            continue;
        }
        match flag {
            0x08 => rel_path = Some(val),
            0x10 => work_dir = Some(val),
            0x40 => icon_loc = Some(val),
            _ => {}
        }
    }

    if let (Some(ref wd), Some(ref rp)) = (&work_dir, &rel_path) {
        let joined = format!("{}\\{}", wd.trim_end_matches('\\'), rp);
        out.push(joined);
    }
    if let Some(rp) = rel_path {
        out.push(rp);
    }

    // 4. ExtraData blocks (EnvironmentVariableDataBlock = 0xA0000001)
    while pos + 8 <= bytes.len() {
        let Some(blk_size) = read_u32(pos) else {
            break;
        };
        if blk_size < 8 {
            break;
        }
        let blk_end = pos.saturating_add(blk_size as usize).min(bytes.len());
        let blk_sig = read_u32(pos + 4).unwrap_or(0);
        if blk_sig == 0xA000_0001 && blk_end >= pos + 0x110 {
            if let Some(u) = read_cstr_utf16(pos + 0x10C, blk_end) {
                out.push(u);
            }
            if let Some(a) = read_cstr_ansi(pos + 8, (pos + 0x10C).min(blk_end)) {
                if !out.contains(&a) {
                    out.push(a);
                }
            }
        }
        pos = blk_end;
    }

    if let Some(ic) = icon_loc {
        if ic.to_ascii_lowercase().contains(".exe") && !out.contains(&ic) {
            out.push(ic);
        }
    }

    out
}

/// Extract custom `IconLocation` (`0x40`) and `IconEnvironmentDataBlock` (`0xA0000007`) paths from a `.lnk` file.
fn parse_shllink_icon_locations(bytes: &[u8]) -> Vec<String> {
    let mut icons = Vec::new();
    if bytes.len() < 0x4C || &bytes[0..4] != b"\x4C\x00\x00\x00" {
        return icons;
    }
    let read_u16 = |off: usize| -> Option<u16> {
        let s = bytes.get(off..off + 2)?;
        Some(u16::from_le_bytes([s[0], s[1]]))
    };
    let read_u32 = |off: usize| -> Option<u32> {
        let s = bytes.get(off..off + 4)?;
        Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    };
    let read_cstr_ansi = |off: usize, limit: usize| -> Option<String> {
        if off >= limit || limit > bytes.len() {
            return None;
        }
        let slice = &bytes[off..limit];
        let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
        let s = String::from_utf8_lossy(&slice[..end]).trim().to_string();
        (!s.is_empty()).then_some(s)
    };
    let read_cstr_utf16 = |off: usize, limit: usize| -> Option<String> {
        if off + 2 > limit || limit > bytes.len() {
            return None;
        }
        let mut u16s = Vec::new();
        let mut p = off;
        while p + 2 <= limit {
            let ch = u16::from_le_bytes([bytes[p], bytes[p + 1]]);
            if ch == 0 {
                break;
            }
            u16s.push(ch);
            p += 2;
        }
        let s = String::from_utf16_lossy(&u16s).trim().to_string();
        (!s.is_empty()).then_some(s)
    };

    let Some(link_flags) = read_u32(0x14) else {
        return icons;
    };
    let mut pos: usize = 0x4C;

    if link_flags & 0x01 != 0 {
        let Some(id_list_size) = read_u16(pos) else {
            return icons;
        };
        pos = pos.saturating_add(2 + id_list_size as usize);
    }
    if link_flags & 0x02 != 0 {
        let Some(info_size) = read_u32(pos) else {
            return icons;
        };
        pos = pos.saturating_add(info_size as usize);
    }

    let is_unicode = (link_flags & 0x80) != 0;
    let mut work_dir: Option<String> = None;
    let mut icon_loc: Option<String> = None;

    for flag in [0x04u32, 0x08, 0x10, 0x20, 0x40] {
        if link_flags & flag == 0 {
            continue;
        }
        let Some(char_count) = read_u16(pos) else {
            break;
        };
        pos += 2;
        let byte_len = if is_unicode {
            (char_count as usize) * 2
        } else {
            char_count as usize
        };
        let Some(slice) = bytes.get(pos..pos + byte_len) else {
            break;
        };
        pos += byte_len;

        let val = if is_unicode {
            let u16s: Vec<u16> = slice
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&u16s).trim().to_string()
        } else {
            String::from_utf8_lossy(slice).trim().to_string()
        };
        if val.is_empty() {
            continue;
        }
        match flag {
            0x10 => work_dir = Some(val),
            0x40 => icon_loc = Some(val),
            _ => {}
        }
    }

    if let Some(ic) = icon_loc {
        if let Some(ref wd) = work_dir {
            if !ic.contains(':') && !ic.starts_with('%') && !ic.starts_with('\\') {
                icons.push(format!("{}\\{}", wd.trim_end_matches('\\'), ic));
            }
        }
        icons.push(ic);
    }

    // Check IconEnvironmentDataBlock (signature = 0xA0000007)
    while pos + 8 <= bytes.len() {
        let Some(blk_size) = read_u32(pos) else {
            break;
        };
        if blk_size < 8 {
            break;
        }
        let blk_end = pos.saturating_add(blk_size as usize).min(bytes.len());
        let blk_sig = read_u32(pos + 4).unwrap_or(0);
        if blk_sig == 0xA000_0007 && blk_end >= pos + 0x110 {
            if let Some(u) = read_cstr_utf16(pos + 0x10C, blk_end) {
                if !icons.contains(&u) {
                    icons.push(u);
                }
            }
            if let Some(a) = read_cstr_ansi(pos + 8, (pos + 0x10C).min(blk_end)) {
                if !icons.contains(&a) {
                    icons.push(a);
                }
            }
        }
        pos = blk_end;
    }

    icons
}

fn meaningful_title_tokens(title: &str) -> Vec<String> {
    title
        .split(|c: char| !c.is_alphanumeric())
        .map(|w| w.trim().to_lowercase())
        .filter(|w| {
            w.len() >= 3
                && !matches!(
                    w.as_str(),
                    "the" | "and" | "for" | "edition" | "game" | "online" | "launcher" | "client"
                )
        })
        .collect()
}

fn text_matches_title_tokens(text: &str, tokens: &[String]) -> bool {
    if tokens.is_empty() {
        return false;
    }
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .map(|w| w.trim().to_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    let compact: String = words.concat();

    for tok in tokens {
        if words.iter().any(|w| w == tok) {
            return true;
        }
        if tok.len() >= 4 && compact.contains(tok) {
            return true;
        }
    }
    false
}

/// Choose the most accurate icon source (`.ico`, `.png`, or `.exe`) after running a Setup or SingleExe installer:
/// 1. Explicit `.lnk` shortcut `IconLocation` inside `pfx/drive_c` (Desktop / Start Menu).
/// 2. Explicit `DisplayIcon` (`.ico` / `.png` / game `.exe`) in Wine Registry `Uninstall` keys.
/// 3. Game-named `.ico` file inside the installed directory (`main_exe.parent()`).
/// 4. Game-branded launcher bootstrapper (`installer_exe`) when `main_exe` is a shared umbrella launcher
///    whose filename, folder, and PE metadata have zero overlap with `final_name`.
/// 5. Default: `main_exe`.
pub fn resolve_installed_icon_source(
    container_dir: &Path,
    main_exe: &Path,
    installer_exe: Option<&Path>,
    final_name: &str,
) -> PathBuf {
    let pfx_drive_c = container_dir.join("pfx/drive_c");
    let title_tokens = meaningful_title_tokens(final_name);

    // 1. Check Desktop and Start Menu `.lnk` shortcuts for an explicit `IconLocation`
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

        let mut lnk_icons: Vec<(i64, PathBuf)> = Vec::new();
        for sdir in shortcut_dirs {
            if !sdir.is_dir() {
                continue;
            }
            for entry in WalkDir::new(&sdir).max_depth(4).into_iter().flatten() {
                let p = entry.path();
                if !p
                    .extension()
                    .and_then(|e| e.to_str())
                    .map_or(false, |e| e.eq_ignore_ascii_case("lnk"))
                {
                    continue;
                }
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                let stem_lower = stem.to_lowercase();
                if stem_lower.contains("uninstall") || stem_lower.contains("unins") {
                    continue;
                }

                let Ok(bytes) = fs::read(p) else {
                    continue;
                };
                let target_matches = resolve_lnk_to_linux_path(p, &pfx_drive_c)
                    .map(|t| t == main_exe)
                    .unwrap_or(false);
                let name_matches = text_matches_title_tokens(stem, &title_tokens);

                for raw_ic in parse_shllink_icon_locations(&bytes) {
                    if let Some(ic_path) = resolve_windows_path_to_linux(&raw_ic, &pfx_drive_c) {
                        if ic_path != main_exe
                            && !is_auxiliary_exe(&ic_path)
                            && icon_source_dimensions(&ic_path).is_some()
                        {
                            let mut prio = if sdir.ends_with("Desktop") { 1000 } else { 500 };
                            if target_matches {
                                prio += 500;
                            }
                            if name_matches {
                                prio += 300;
                            }
                            lnk_icons.push((prio, ic_path));
                        }
                    }
                }
            }
        }

        lnk_icons.sort_by(|a, b| b.0.cmp(&a.0));
        if let Some((_, best_ic)) = lnk_icons.into_iter().next() {
            return best_ic;
        }
    }

    // 2. Check Wine Registry `Uninstall` `DisplayIcon` entries for custom `.ico` / `.png` icons
    let pfx_dir = container_dir.join("pfx");
    for reg_name in ["user.reg", "system.reg"] {
        let reg_path = pfx_dir.join(reg_name);
        let Ok(content) = fs::read_to_string(&reg_path) else {
            continue;
        };
        let mut in_uninstall = false;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                let lower = trimmed.to_ascii_lowercase();
                in_uninstall =
                    lower.contains("currentversion\\\\uninstall\\\\") && !lower.ends_with("uninstall]");
                continue;
            }
            if !in_uninstall {
                continue;
            }
            if let Some((k, v)) = parse_wine_reg_key_value(trimmed) {
                if k.eq_ignore_ascii_case("DisplayIcon") {
                    if let Some(ic_path) = resolve_windows_path_to_linux(&v, &pfx_drive_c) {
                        let is_ico_or_png = ic_path
                            .extension()
                            .and_then(|e| e.to_str())
                            .map_or(false, |e| {
                                e.eq_ignore_ascii_case("ico") || e.eq_ignore_ascii_case("png")
                            });
                        if ic_path != main_exe
                            && is_ico_or_png
                            && icon_source_dimensions(&ic_path).is_some()
                        {
                            return ic_path;
                        }
                    }
                }
            }
        }
    }

    // 3. Check for a game-named `.ico` file inside `main_exe`'s installation folder
    if let Some(main_dir) = main_exe.parent() {
        if !is_protected_directory(main_dir) && !title_tokens.is_empty() {
            for entry in WalkDir::new(main_dir).max_depth(3).into_iter().flatten() {
                let p = entry.path();
                if !p.is_file() {
                    continue;
                }
                if p.extension()
                    .and_then(|e| e.to_str())
                    .map_or(false, |e| e.eq_ignore_ascii_case("ico"))
                {
                    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    if text_matches_title_tokens(stem, &title_tokens)
                        && icon_source_dimensions(p).is_some()
                    {
                        return p.to_path_buf();
                    }
                }
            }
        }
    }

    // 4. Umbrella publisher launcher check:
    // If `main_exe` (filename, parent folder, and PE metadata) does NOT match `final_name`,
    // while `installer_exe` is a game-branded bootstrapper matching `final_name` with a high-res icon,
    // preserve the game's icon from `installer_exe`.
    if let Some(inst_exe) = installer_exe {
        if inst_exe != main_exe && inst_exe.is_file() && !title_tokens.is_empty() {
            let main_stem = main_exe
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            let main_parent = main_exe
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("");
            let main_meta = extract_exe_metadata(main_exe);

            let mut main_matches_game = text_matches_title_tokens(main_stem, &title_tokens)
                || text_matches_title_tokens(main_parent, &title_tokens);
            if !main_matches_game {
                for sig in main_meta.candidate_signals() {
                    if text_matches_title_tokens(&sig, &title_tokens) {
                        main_matches_game = true;
                        break;
                    }
                }
            }

            if !main_matches_game {
                let inst_fname_lower = inst_exe
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                let is_bare_setup_name = matches!(
                    inst_fname_lower.as_str(),
                    "setup.exe" | "install.exe" | "installer.exe" | "autorun.exe"
                ) || inst_fname_lower.ends_with(".msi");

                let inst_parent = inst_exe.parent().unwrap_or_else(|| Path::new("/tmp"));
                let has_multipart_bins = inst_parent.join("setup-1.bin").exists()
                    || inst_parent.join("data-1.bin").exists()
                    || inst_parent.join("data1.cab").exists();

                if !is_bare_setup_name && !has_multipart_bins {
                    let inst_stem = inst_exe
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    let inst_meta = extract_exe_metadata(inst_exe);
                    let mut inst_matches_game = text_matches_title_tokens(inst_stem, &title_tokens);
                    if !inst_matches_game {
                        for sig in inst_meta.candidate_signals() {
                            if text_matches_title_tokens(&sig, &title_tokens) {
                                inst_matches_game = true;
                                break;
                            }
                        }
                    }

                    if inst_matches_game {
                        if let Some((w, h)) = icon_source_dimensions(inst_exe) {
                            if w >= 48 && h >= 48 {
                                return inst_exe.to_path_buf();
                            }
                        }
                    }
                }
            }
        }
    }

    main_exe.to_path_buf()
}

fn join_win_base_and_suffix(base: &str, suffix: &str) -> String {
    if base.is_empty() {
        return suffix.to_string();
    }
    if suffix.is_empty() {
        return base.to_string();
    }
    if base.ends_with('\\') || suffix.starts_with('\\') {
        format!("{base}{suffix}")
    } else {
        format!("{base}\\{suffix}")
    }
}

fn extract_win_exe_substrings(s: &str) -> Vec<String> {
    let mut results = Vec::new();
    let lower = s.to_ascii_lowercase();
    let mut search_from = 0usize;

    while let Some(rel_exe) = lower[search_from..].find(".exe") {
        let exe_end = search_from + rel_exe + 4;
        let prefix = &s[..exe_end];
        let prefix_bytes = prefix.as_bytes();

        // Find rightmost drive prefix like `C:\` or `Z:\` before `.exe`
        let mut found_start = None;
        if prefix_bytes.len() >= 3 {
            for i in (0..=prefix_bytes.len() - 3).rev() {
                if (prefix_bytes[i] == b'c'
                    || prefix_bytes[i] == b'C'
                    || prefix_bytes[i] == b'z'
                    || prefix_bytes[i] == b'Z')
                    && prefix_bytes[i + 1] == b':'
                    && (prefix_bytes[i + 2] == b'\\' || prefix_bytes[i + 2] == b'/')
                {
                    if s.is_char_boundary(i) {
                        found_start = Some(i);
                        break;
                    }
                }
            }
        }

        if let Some(start) = found_start {
            let sub = s[start..exe_end].to_string();
            if !results.contains(&sub) {
                results.push(sub);
            }
        }
        search_from = exe_end;
    }

    results
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
    if current.len() >= 5 {
        if let Ok(s) = String::from_utf8(current) {
            out.push(s);
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
        if current.len() >= 5 {
            if let Ok(s) = String::from_utf16(&current) {
                out.push(s);
            }
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

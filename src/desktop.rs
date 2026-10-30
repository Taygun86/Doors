use crate::detector::DoorsGameConfig;
use crate::disks::real_user_home;
use crate::i18n::tr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const APP_ID: &str = "io.github.doors.Doors";

/// Returns the path to the user's Desktop directory (~/Desktop, ~/Masaüstü, etc.)
pub fn get_desktop_dir() -> PathBuf {
    let home = real_user_home();
    // 1. Check ~/.config/user-dirs.dirs for XDG_DESKTOP_DIR
    let user_dirs_file = home.join(".config/user-dirs.dirs");
    if let Ok(content) = fs::read_to_string(&user_dirs_file) {
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("XDG_DESKTOP_DIR=") {
                let unquoted = rest.trim_matches('"').trim_matches('\'');
                let path = if let Some(sub) = unquoted.strip_prefix("$HOME/") {
                    home.join(sub)
                } else if unquoted.starts_with('/') {
                    PathBuf::from(unquoted)
                } else {
                    home.join(unquoted)
                };
                return path;
            }
        }
    }

    // 2. Try dirs::desktop_dir()
    if let Some(d) = dirs::desktop_dir() {
        return d;
    }

    // 3. Fallback to common folder names if they exist
    for name in &["Desktop", "Masaüstü"] {
        let p = home.join(name);
        if p.is_dir() {
            return p;
        }
    }

    home.join("Desktop")
}

pub fn sanitize_desktop_filename(name: &str) -> String {
    let mut clean: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '-',
            c => c,
        })
        .collect();

    clean = clean.trim().trim_matches('.').trim().to_string();
    if clean.is_empty() {
        "game".to_string()
    } else {
        clean
    }
}

pub fn has_desktop_folder_shortcut(cfg: &DoorsGameConfig) -> bool {
    let desktop_dir = get_desktop_dir();
    let clean_name = sanitize_desktop_filename(&cfg.name);
    desktop_dir.join(format!("{}.desktop", clean_name)).exists()
        || desktop_dir.join(format!("doors-{}.desktop", cfg.slug)).exists()
}

pub fn create_desktop_folder_shortcut(cfg: &DoorsGameConfig) -> anyhow::Result<PathBuf> {
    let desktop_dir = get_desktop_dir();
    fs::create_dir_all(&desktop_dir)?;

    let (desktop_content, _) = generate_desktop_file_content(cfg);
    let clean_name = sanitize_desktop_filename(&cfg.name);
    let desktop_file = desktop_dir.join(format!("{}.desktop", clean_name));

    // Also remove any legacy doors-{slug}.desktop on the desktop folder if present
    let legacy_file = desktop_dir.join(format!("doors-{}.desktop", cfg.slug));
    if legacy_file != desktop_file && legacy_file.exists() {
        let _ = fs::remove_file(legacy_file);
    }

    fs::write(&desktop_file, desktop_content)?;
    if let Ok(meta) = fs::metadata(&desktop_file) {
        let mut perms = meta.permissions();
        perms.set_mode(0o755);
        let _ = fs::set_permissions(&desktop_file, perms);
    }

    trust_desktop_file(&desktop_file);
    Ok(desktop_file)
}

pub fn remove_desktop_folder_shortcut(cfg: &DoorsGameConfig) {
    let desktop_dir = get_desktop_dir();
    let clean_name = sanitize_desktop_filename(&cfg.name);
    let _ = fs::remove_file(desktop_dir.join(format!("{}.desktop", clean_name)));
    let _ = fs::remove_file(desktop_dir.join(format!("doors-{}.desktop", cfg.slug)));
}

fn trust_desktop_file(path: &Path) {
    use std::process::{Command, Stdio};
    let in_flatpak = Path::new("/.flatpak-info").exists();
    if in_flatpak {
        let _ = Command::new("flatpak-spawn")
            .arg("--host")
            .arg("gio")
            .arg("set")
            .arg(path)
            .arg("metadata::trusted")
            .arg("true")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    } else {
        let _ = Command::new("gio")
            .arg("set")
            .arg(path)
            .arg("metadata::trusted")
            .arg("true")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

fn generate_desktop_file_content(cfg: &DoorsGameConfig) -> (String, String) {
    let home = real_user_home();
    let icons_dir = home.join(".local/share/icons/hicolor/256x256/apps");
    let _ = fs::create_dir_all(&icons_dir);

    let icon_value = if let Some(ref png_path) = cfg.icon_png {
        if png_path.is_file() {
            let dest_icon = icons_dir.join(format!("doors-{}.png", cfg.slug));
            let _ = fs::copy(png_path, &dest_icon);
            png_path.display().to_string()
        } else {
            "input-gaming-symbolic".to_string()
        }
    } else {
        "input-gaming-symbolic".to_string()
    };

    let in_flatpak = Path::new("/.flatpak-info").exists();
    let exec_cmd = if in_flatpak {
        format!(
            "flatpak run {} \"{}\"",
            APP_ID,
            cfg.main_exe.display()
        )
    } else {
        let self_exe = std::env::current_exe().unwrap_or_else(|_| "doors".into());
        format!(
            "\"{}\" \"{}\"",
            self_exe.display(),
            cfg.main_exe.display()
        )
    };

    let work_dir = cfg
        .main_exe
        .parent()
        .unwrap_or(&cfg.container_dir)
        .display();

    let desktop_content = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={name}\n\
         Comment={comment} ({proton})\n\
         Exec={exec}\n\
         Path={path}\n\
         Icon={icon}\n\
         Terminal=false\n\
         Categories=Game;Application;\n\
         StartupNotify=true\n",
        name = cfg.name,
        comment = tr("Run with Doors"),
        proton = cfg.proton_name,
        exec = exec_cmd,
        path = work_dir,
        icon = icon_value,
    );

    (desktop_content, icon_value)
}

/// Save `doors.json` inside `cfg.container_dir` and generate the `.desktop` shortcut + icon.
pub fn save_config_and_create_desktop_entry(cfg: &DoorsGameConfig) -> anyhow::Result<()> {
    fs::create_dir_all(&cfg.container_dir)?;
    let json_path = cfg.container_dir.join("doors.json");
    let json_str = serde_json::to_string_pretty(cfg)?;
    fs::write(&json_path, json_str)?;

    create_desktop_shortcut(cfg)?;

    if cfg.desktop_shortcut {
        let _ = create_desktop_folder_shortcut(cfg);
    } else {
        remove_desktop_folder_shortcut(cfg);
    }

    Ok(())
}

pub fn create_desktop_shortcut(cfg: &DoorsGameConfig) -> anyhow::Result<()> {
    let home = real_user_home();
    let apps_dir = home.join(".local/share/applications");
    fs::create_dir_all(&apps_dir)?;

    let (desktop_content, _) = generate_desktop_file_content(cfg);
    let desktop_file = apps_dir.join(format!("doors-{}.desktop", cfg.slug));
    fs::write(&desktop_file, desktop_content)?;
    if let Ok(meta) = fs::metadata(&desktop_file) {
        let mut perms = meta.permissions();
        perms.set_mode(0o755);
        let _ = fs::set_permissions(&desktop_file, perms);
    }

    refresh_desktop_database(&apps_dir);
    Ok(())
}

pub fn remove_desktop_shortcut(cfg: &DoorsGameConfig) {
    let home = real_user_home();
    let apps_dir = home.join(".local/share/applications");
    let desktop_file = apps_dir.join(format!("doors-{}.desktop", cfg.slug));
    let icon_file = home
        .join(".local/share/icons/hicolor/256x256/apps")
        .join(format!("doors-{}.png", cfg.slug));
    let _ = fs::remove_file(desktop_file);
    let _ = fs::remove_file(icon_file);
    remove_desktop_folder_shortcut(cfg);
    refresh_desktop_database(&apps_dir);
}

fn refresh_desktop_database(apps_dir: &Path) {
    use std::process::{Command, Stdio};
    if Path::new("/.flatpak-info").exists() {
        let _ = Command::new("flatpak-spawn")
            .arg("--host")
            .arg("update-desktop-database")
            .arg(apps_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    } else {
        let _ = Command::new("update-desktop-database")
            .arg(apps_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_desktop_filename() {
        assert_eq!(sanitize_desktop_filename("Star Shift"), "Star Shift");
        assert_eq!(sanitize_desktop_filename("Game: Remastered / Definitive*"), "Game- Remastered - Definitive-");
        assert_eq!(sanitize_desktop_filename("...Special Edition..."), "Special Edition");
        assert_eq!(sanitize_desktop_filename("   "), "game");
    }
}


use crate::detector::DoorsGameConfig;
use crate::disks::real_user_home;
use crate::i18n::tr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub const APP_ID: &str = "io.github.doors.Doors";

/// Save `doors.json` inside `cfg.container_dir` and generate the `.desktop` shortcut + icon.
pub fn save_config_and_create_desktop_entry(cfg: &DoorsGameConfig) -> anyhow::Result<()> {
    fs::create_dir_all(&cfg.container_dir)?;
    let json_path = cfg.container_dir.join("doors.json");
    let json_str = serde_json::to_string_pretty(cfg)?;
    fs::write(&json_path, json_str)?;

    create_desktop_shortcut(cfg)?;
    Ok(())
}

pub fn create_desktop_shortcut(cfg: &DoorsGameConfig) -> anyhow::Result<()> {
    let home = real_user_home();
    let apps_dir = home.join(".local/share/applications");
    let icons_dir = home.join(".local/share/icons/hicolor/256x256/apps");
    fs::create_dir_all(&apps_dir)?;
    fs::create_dir_all(&icons_dir)?;

    let icon_value = if let Some(ref png_path) = cfg.icon_png {
        if png_path.is_file() {
            let dest_icon = icons_dir.join(format!("doors-{}.png", cfg.slug));
            let _ = fs::copy(png_path, &dest_icon);
            png_path.display().to_string()
        } else {
            "application-x-executable".to_string()
        }
    } else {
        "application-x-executable".to_string()
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
         Categories=Utility;Application;\n\
         StartupNotify=true\n",
        name = cfg.name,
        comment = tr("Run with Doors"),
        proton = cfg.proton_name,
        exec = exec_cmd,
        path = work_dir,
        icon = icon_value,
    );

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


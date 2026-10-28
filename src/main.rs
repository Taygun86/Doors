mod desktop;
mod detector;
mod disks;
mod i18n;
mod icon;
mod installer;
mod proton;
mod ui;

use desktop::APP_ID;
use gtk4::prelude::*;
use libadwaita as adw;

fn main() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN | gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(|app| {
        ui::present_library_window(app);
    });

    app.connect_open(|app, files, _hint| {
        if let Some(first) = files.first() {
            if let Some(path) = first.path() {
                ui::handle_exe_launch(app, path);
                return;
            }
        }
        ui::present_library_window(app);
    });

    let args: Vec<String> = std::env::args().collect();
    // Support `doors /path/to/file.exe` cleanly
    app.run_with_args(&args)
}

#[cfg(test)]
mod tests {
    use crate::detector::{clean_game_title, detect_main_executable, InstallMode, InstallPlan};
    use std::fs;

    #[test]
    fn test_clean_game_title() {
        assert_eq!(clean_game_title("Schedule I-SteamRIP.com"), "Schedule I");
        assert_eq!(
            clean_game_title("Hollow_Knight_[SteamRIP.com]"),
            "Hollow Knight"
        );
        assert_eq!(clean_game_title("setup_cyberpunk"), "cyberpunk");
        assert_eq!(
            clean_game_title("GenshinImpact_install_ua_ef67ba8459b7"),
            "GenshinImpact"
        );
    }

    #[test]
    fn test_analyze_steamrip_and_setup() {
        let tmp = std::env::temp_dir().join("doors_test_analyze");
        let _ = fs::remove_dir_all(&tmp);
        let steamrip_dir = tmp.join("Subnautica-SteamRIP.com");
        fs::create_dir_all(&steamrip_dir).unwrap();
        let game_exe = steamrip_dir.join("Subnautica.exe");
        fs::write(&game_exe, b"MZfake").unwrap();

        let plan = InstallPlan::analyze(&game_exe);
        assert_eq!(plan.game_name, "Subnautica");
        assert!(matches!(plan.mode, InstallMode::PortableFolder { .. }));

        let setup_exe = steamrip_dir.join("setup.exe");
        fs::write(&setup_exe, b"MZsetup").unwrap();
        let setup_plan = InstallPlan::analyze(&setup_exe);
        assert!(matches!(setup_plan.mode, InstallMode::SetupInstaller { .. }));

        let genshin_installer = tmp.join("GenshinImpact_install_ua_ef67ba8459b7.exe");
        fs::write(&genshin_installer, b"MZinstaller").unwrap();
        let genshin_plan = InstallPlan::analyze(&genshin_installer);
        assert_eq!(genshin_plan.game_name, "GenshinImpact");
        assert!(matches!(
            genshin_plan.mode,
            InstallMode::SetupInstaller { .. }
        ));

        let container_dir = tmp.join("container");
        let hoyo_dir = container_dir.join("pfx/drive_c/Program Files/HoYoPlay");
        let steam_dir = container_dir.join("pfx/drive_c/Program Files (x86)/Steam");
        fs::create_dir_all(&hoyo_dir).unwrap();
        fs::create_dir_all(&steam_dir).unwrap();
        fs::write(steam_dir.join("steam.exe"), vec![0u8; 500_000]).unwrap();
        fs::write(hoyo_dir.join("7z.exe"), vec![0u8; 600_000]).unwrap();
        fs::write(hoyo_dir.join("uninst.exe"), vec![0u8; 400_000]).unwrap();
        let launcher_exe = hoyo_dir.join("launcher.exe");
        fs::write(&launcher_exe, vec![0u8; 300_000]).unwrap();

        let detected = detect_main_executable(&container_dir, None, "GenshinImpact");
        assert_eq!(detected.map(|(p, _)| p), Some(launcher_exe));

        let _ = fs::remove_dir_all(&tmp);
    }
}

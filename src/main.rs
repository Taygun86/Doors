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
        assert_eq!(clean_game_title("Space_Quest_[v1.2.0]"), "Space Quest");
        assert_eq!(
            clean_game_title("My_Adventure_Game_(x64)"),
            "My Adventure Game"
        );
        assert_eq!(clean_game_title("setup_racing_simulator"), "racing simulator");
        assert_eq!(
            clean_game_title("PixelHero_install_win64_build992"),
            "PixelHero"
        );
    }

    #[test]
    fn test_analyze_portable_and_setup() {
        let tmp = std::env::temp_dir().join("doors_test_analyze");
        let _ = fs::remove_dir_all(&tmp);
        let game_folder = tmp.join("SpaceQuest-Portable");
        fs::create_dir_all(&game_folder).unwrap();
        let game_exe = game_folder.join("SpaceQuest.exe");
        fs::write(&game_exe, b"MZfake").unwrap();

        let plan = InstallPlan::analyze(&game_exe);
        assert_eq!(plan.game_name, "SpaceQuest");
        assert!(matches!(plan.mode, InstallMode::PortableFolder { .. }));

        let setup_exe = game_folder.join("setup.exe");
        fs::write(&setup_exe, b"MZsetup").unwrap();
        let setup_plan = InstallPlan::analyze(&setup_exe);
        assert!(matches!(setup_plan.mode, InstallMode::SetupInstaller { .. }));

        let standalone_installer = tmp.join("ActionGame_install_win64_release.exe");
        fs::write(&standalone_installer, b"MZinstaller").unwrap();
        let standalone_plan = InstallPlan::analyze(&standalone_installer);
        assert_eq!(standalone_plan.game_name, "ActionGame");
        assert!(matches!(
            standalone_plan.mode,
            InstallMode::SetupInstaller { .. }
        ));

        let container_dir = tmp.join("container");
        let launcher_dir = container_dir.join("pfx/drive_c/Program Files/GameLauncher");
        let steam_dir = container_dir.join("pfx/drive_c/Program Files (x86)/Steam");
        fs::create_dir_all(&launcher_dir).unwrap();
        fs::create_dir_all(&steam_dir).unwrap();
        fs::write(steam_dir.join("steam.exe"), vec![0u8; 500_000]).unwrap();
        fs::write(launcher_dir.join("7z.exe"), vec![0u8; 600_000]).unwrap();
        fs::write(launcher_dir.join("uninst.exe"), vec![0u8; 400_000]).unwrap();
        let launcher_exe = launcher_dir.join("launcher.exe");
        fs::write(&launcher_exe, vec![0u8; 300_000]).unwrap();

        let detected = detect_main_executable(&container_dir, None, "ActionGame");
        assert_eq!(detected.map(|(p, _)| p), Some(launcher_exe));

        let _ = fs::remove_dir_all(&tmp);
    }
}

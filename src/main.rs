mod desktop;
mod detector;
mod disks;
mod i18n;
mod icon;
mod installer;
mod proton;
mod proton_manager;
mod ui;
mod umu;

use desktop::APP_ID;
use gtk4::prelude::*;
use libadwaita as adw;

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--update-protons-bg" || a == "--update-protons") {
        eprintln!("Doors: Running background Proton updater...");
        if let Err(e) = proton_manager::run_background_update() {
            eprintln!("Doors: Background update error: {e}");
        }
        umu::sync_umu_database_blocking();
        return glib::ExitCode::SUCCESS;
    }

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
    use crate::detector::{
        clean_game_title, detect_main_executable, sanitize_folder_name, InstallMode, InstallPlan,
    };
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
        assert_eq!(
            clean_game_title("My-Adventure-Game-Portable-Release.com"),
            "My Adventure Game"
        );
        assert_eq!(
            clean_game_title("StudioLauncher_v1.6.0.1607_6_6_subproject"),
            "StudioLauncher subproject"
        );
        assert_eq!(
            clean_game_title("My.Adventure.Game.v1.2.3.x64"),
            "My Adventure Game"
        );
        assert_eq!(clean_game_title("Space_Quest_2_1.4.0"), "Space Quest 2");
        assert_eq!(clean_game_title("İzmir_Yarış_setup"), "İzmir Yarış");
        assert_eq!(
            sanitize_folder_name("Nebula Odyssey: Horizon"),
            "Nebula Odyssey - Horizon"
        );
    }

    #[test]
    fn test_analyze_pe_embedded_metadata_without_filename() {
        let tmp = std::env::temp_dir().join("doors_test_pe_analyze");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let anon_setup = tmp.join("setup.exe");
        let pe_bytes = crate::icon::tests::build_synthetic_pe_exe(
            &[
                ("ProductName", "Celestial Horizon"),
                ("FileDescription", "Celestial Horizon Installer"),
            ],
            None,
        );
        fs::write(&anon_setup, pe_bytes).unwrap();

        let plan = InstallPlan::analyze(&anon_setup);
        assert_eq!(plan.game_name, "Celestial Horizon");
        assert!(matches!(plan.mode, InstallMode::SetupInstaller { .. }));

        let _ = fs::remove_dir_all(&tmp);
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

    #[test]
    fn test_rename_container() {
        use crate::detector::DoorsGameConfig;
        use std::path::PathBuf;

        let tmp = std::env::temp_dir().join("doors_test_rename");
        let _ = fs::remove_dir_all(&tmp);
        let original = tmp.join("Old Game Name");
        fs::create_dir_all(&original).unwrap();
        let main_exe = original.join("bin/game.exe");
        fs::create_dir_all(main_exe.parent().unwrap()).unwrap();
        fs::write(&main_exe, b"test").unwrap();
        let icon = original.join("icon.png");
        fs::write(&icon, b"icon").unwrap();

        let mut cfg = DoorsGameConfig::new(
            "Old Game".into(),
            "old-game".into(),
            original.clone(),
            main_exe.clone(),
            "Proton-GE".into(),
            PathBuf::from("/proton"),
            Some(icon.clone()),
        );

        // Test successful rename
        assert!(cfg.rename_container("New Game Name").is_ok());
        assert_eq!(cfg.container_dir, tmp.join("New Game Name"));
        assert_eq!(cfg.main_exe, tmp.join("New Game Name/bin/game.exe"));
        assert_eq!(cfg.icon_png, Some(tmp.join("New Game Name/icon.png")));
        assert!(!original.exists());
        assert!(cfg.main_exe.exists());

        // Test invalid characters and empty
        assert!(cfg.rename_container("invalid/name").is_err());
        assert!(cfg.rename_container("").is_err());
        assert!(cfg.rename_container("..").is_err());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_lnk_counted_string_and_registry_detection() {
        let tmp = std::env::temp_dir().join("doors_test_lnk_and_reg");
        let _ = fs::remove_dir_all(&tmp);

        // 1. Test MS-SHLLINK `.lnk` with 39-char target path (`0x27 == '\''` length prefix)
        let container_lnk = tmp.join("c_lnk");
        let drive_c = container_lnk.join("pfx/drive_c");
        let target_exe = drive_c.join("Program Files/StudioHub/Launcher.exe");
        let desk_dir = drive_c.join("users/steamuser/Desktop");
        fs::create_dir_all(target_exe.parent().unwrap()).unwrap();
        fs::create_dir_all(&desk_dir).unwrap();
        fs::write(&target_exe, vec![0u8; 250_000]).unwrap();

        let mut lnk_bytes = vec![0u8; 0x4C];
        lnk_bytes[0..4].copy_from_slice(b"\x4C\x00\x00\x00");
        // LinkFlags: HasWorkingDir (0x10) | HasIconLocation (0x40) | IsUnicode (0x80) = 0xD0
        lnk_bytes[0x14..0x18].copy_from_slice(&0x0000_00D0u32.to_le_bytes());

        let push_counted_utf16 = |buf: &mut Vec<u8>, s: &str| {
            let u16s: Vec<u16> = s.encode_utf16().collect();
            buf.extend_from_slice(&(u16s.len() as u16).to_le_bytes());
            for u in u16s {
                buf.extend_from_slice(&u.to_le_bytes());
            }
        };
        push_counted_utf16(&mut lnk_bytes, "C:\\Program Files\\StudioHub");
        // 39 chars -> length prefix 0x0027 ('\''), which previously corrupted UTF-16LE scanning!
        push_counted_utf16(&mut lnk_bytes, "C:\\Program Files\\StudioHub\\Launcher.exe");

        let lnk_path = desk_dir.join("StudioHub.lnk");
        fs::write(&lnk_path, &lnk_bytes).unwrap();

        let resolved = crate::detector::resolve_lnk_to_linux_path(&lnk_path, &drive_c);
        assert_eq!(resolved, Some(target_exe.clone()));
        let detected = detect_main_executable(&container_lnk, None, "StudioHub");
        assert_eq!(detected.map(|(p, _)| p), Some(target_exe));

        // 2. Test Wine Registry `system.reg` Uninstall detection & deep AppData/Local/Programs (depth 7)
        let container_reg = tmp.join("c_reg");
        let deep_exe = container_reg
            .join("pfx/drive_c/users/steamuser/AppData/Local/Programs/NebulaHub/bin/NebulaClient.exe");
        fs::create_dir_all(deep_exe.parent().unwrap()).unwrap();
        fs::write(&deep_exe, vec![0u8; 300_000]).unwrap();

        let detected_deep = detect_main_executable(&container_reg, None, "Nebula");
        assert_eq!(detected_deep.map(|(p, _)| p), Some(deep_exe));

        let container_z = tmp.join("c_reg_z");
        fs::create_dir_all(container_z.join("pfx/drive_c")).unwrap();
        let custom_host_dir = tmp.join("custom_host_install");
        fs::create_dir_all(&custom_host_dir).unwrap();
        let custom_exe = custom_host_dir.join("Launcher.exe");
        fs::write(&custom_exe, vec![0u8; 320_000]).unwrap();

        let win_z_path = format!(
            "Z:{}",
            custom_exe.display().to_string().replace('/', "\\\\")
        );
        let reg_content = format!(
            "WINE REGISTRY Version 2\n\n\
             [Software\\\\Microsoft\\\\Windows\\\\CurrentVersion\\\\Uninstall\\\\CustomApp] 1730000000\n\
             \"DisplayName\"=\"Custom Horizon\"\n\
             \"DisplayIcon\"=\"\\\"{}\\\",0\"\n",
            win_z_path
        );
        fs::write(container_z.join("pfx/system.reg"), reg_content).unwrap();

        let detected_z = detect_main_executable(&container_z, None, "Custom Horizon");
        assert_eq!(detected_z.map(|(p, _)| p), Some(custom_exe));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_proton_arch_filtering() {
        if cfg!(target_arch = "x86_64") {
            assert!(!crate::proton_manager::is_compatible_host_archive(
                "proton-cachyos-11.0-20261015-slr-arm64.tar.xz"
            ));
            assert!(!crate::proton_manager::is_compatible_host_archive(
                "proton-cachyos-11.0-20261015-slr-x86_64_v3.tar.xz"
            ));
            assert!(crate::proton_manager::is_compatible_host_archive(
                "proton-cachyos-11.0-20261015-slr-x86_64.tar.xz"
            ));

            let tmp = std::env::temp_dir().join("doors_test_elf_arch");
            let _ = fs::remove_dir_all(&tmp);

            let bad_dir = tmp.join("proton-custom-11.0");
            fs::create_dir_all(bad_dir.join("files/bin")).unwrap();
            let mut arm64_elf = vec![0u8; 24];
            arm64_elf[0..4].copy_from_slice(b"\x7FELF");
            arm64_elf[18..20].copy_from_slice(&0xB7u16.to_le_bytes()); // EM_AARCH64
            fs::write(bad_dir.join("files/bin/wine64"), &arm64_elf).unwrap();
            assert!(!crate::proton::is_proton_dir_compatible_with_host(&bad_dir));

            let good_dir = tmp.join("GE-Proton10-1");
            fs::create_dir_all(good_dir.join("files/bin")).unwrap();
            let mut x64_elf = vec![0u8; 24];
            x64_elf[0..4].copy_from_slice(b"\x7FELF");
            x64_elf[18..20].copy_from_slice(&0x3Eu16.to_le_bytes()); // EM_X86_64
            fs::write(good_dir.join("files/bin/wine64"), &x64_elf).unwrap();
            assert!(crate::proton::is_proton_dir_compatible_with_host(&good_dir));

            let _ = fs::remove_dir_all(&tmp);
        }
    }

    #[test]
    fn test_resolve_installed_icon_source_lnk_and_bootstrapper() {
        let tmp = std::env::temp_dir().join("doors_test_icon_source");
        let _ = fs::remove_dir_all(&tmp);

        let container = tmp.join("container");
        let hub_dir = container.join("pfx/drive_c/Program Files/StudioHub");
        let desktop_dir = container.join("pfx/drive_c/users/steamuser/Desktop");
        fs::create_dir_all(&hub_dir).unwrap();
        fs::create_dir_all(&desktop_dir).unwrap();

        let hub_exe = hub_dir.join("StudioHub.exe");
        fs::write(&hub_exe, vec![0u8; 1024]).unwrap();

        // Build a valid 64x64 ICO file for the shortcut's custom IconLocation
        let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
        let img = ico::IconImage::from_rgba_data(64, 64, vec![255u8; 64 * 64 * 4]);
        icon_dir.add_entry(ico::IconDirEntry::encode(&img).unwrap());
        let custom_ico = hub_dir.join("subproject_custom.ico");
        let mut ico_file = fs::File::create(&custom_ico).unwrap();
        icon_dir.write(&mut ico_file).unwrap();

        // Build a synthetic MS-SHLLINK (.lnk) with HasRelativePath (0x08) | HasIconLocation (0x40) | IsUnicode (0x80)
        let mut lnk = vec![0u8; 0x4C];
        lnk[0..4].copy_from_slice(b"\x4C\x00\x00\x00");
        lnk[0x14..0x18].copy_from_slice(&(0x08u32 | 0x40u32 | 0x80u32).to_le_bytes());

        let rel_target = "C:\\Program Files\\StudioHub\\StudioHub.exe";
        let rel_u16: Vec<u16> = rel_target.encode_utf16().collect();
        lnk.extend_from_slice(&(rel_u16.len() as u16).to_le_bytes());
        for ch in rel_u16 {
            lnk.extend_from_slice(&ch.to_le_bytes());
        }

        let icon_loc = "C:\\Program Files\\StudioHub\\subproject_custom.ico,0";
        let ic_u16: Vec<u16> = icon_loc.encode_utf16().collect();
        lnk.extend_from_slice(&(ic_u16.len() as u16).to_le_bytes());
        for ch in ic_u16 {
            lnk.extend_from_slice(&ch.to_le_bytes());
        }

        let lnk_path = desktop_dir.join("Nebula Horizon.lnk");
        fs::write(&lnk_path, &lnk).unwrap();

        // 1. `.lnk` custom `IconLocation` takes top priority and extracts cleanly to PNG
        let picked = crate::detector::resolve_installed_icon_source(
            &container,
            &hub_exe,
            None,
            "Nebula Horizon",
        );
        assert_eq!(picked, custom_ico);
        let out_png = tmp.join("extracted.png");
        assert!(crate::icon::extract_exe_icon_to_png(&picked, None, &out_png));
        assert!(out_png.is_file());

        // 2. When no `.lnk` custom icon exists and `main_exe` is an umbrella launcher unrelated to
        //    `final_name`, a game-branded installer with >= 48x48 icon preserves the game icon.
        let _ = fs::remove_file(&lnk_path);
        let _ = fs::remove_file(&custom_ico);

        let png_bytes = fs::read(&out_png).unwrap();
        let branded_installer = tmp.join("StudioHub_v1.6.0_horizon.png");
        fs::write(&branded_installer, &png_bytes).unwrap();

        let picked_from_bootstrapper = crate::detector::resolve_installed_icon_source(
            &container,
            &hub_exe,
            Some(&branded_installer),
            "Nebula Horizon",
        );
        assert_eq!(picked_from_bootstrapper, branded_installer);

        let _ = fs::remove_dir_all(&tmp);
    }
}

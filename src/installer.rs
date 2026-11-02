use crate::desktop::save_config_and_create_desktop_entry;
use crate::detector::{
    clean_game_title, detect_main_executable, resolve_installed_icon_source, sanitize_folder_name,
    slugify, DoorsGameConfig, InstallMode, InstallPlan,
};
use crate::disks::DiskInfo;
use crate::i18n::tr;
use crate::icon::{
    extract_exe_icon_to_png, extract_exe_metadata, is_generic_metadata_boilerplate,
};
use crate::proton::{spawn_with_proton_config, wait_for_prefix_wineserver, ProtonTool};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub fn execute_install<F>(
    plan: &InstallPlan,
    disk: &DiskInfo,
    proton: &ProtonTool,
    run_after_install: bool,
    desktop_shortcut: bool,
    use_umu: bool,
    umu_id: Option<String>,
    mut on_progress: F,
) -> anyhow::Result<DoorsGameConfig>
where
    F: FnMut(f64, String),
{
    let folder_name = sanitize_folder_name(&plan.game_name);
    let container_dir = disk.doors_root.join(&folder_name);
    let pfx_dir = container_dir.join("pfx");
    let game_dir = container_dir.join("game");

    let initial_umu_store = plan
        .umu_match
        .as_ref()
        .map(|m| m.store.clone())
        .unwrap_or_default();

    fs::create_dir_all(&pfx_dir)?;

    match &plan.mode {
        InstallMode::PortableFolder {
            source_dir,
            clicked_rel_exe,
        } => {
            on_progress(
                0.05,
                format!("{} {}", tr("Preparing folder:"), plan.game_name),
            );

            if source_dir != &game_dir {
                move_directory_with_progress(source_dir, &game_dir, &mut on_progress)?;
            }

            on_progress(0.85, tr("Detecting main executable (.exe)..."));
            let preferred_exe = game_dir.join(clicked_rel_exe);
            let (main_exe, _) =
                detect_main_executable(&container_dir, Some(&preferred_exe), &plan.game_name)
                    .unwrap_or((preferred_exe, None));

            on_progress(0.92, tr("Extracting icon (.ico -> .png)..."));
            let icon_png_path = container_dir.join("icon.png");
            let has_icon =
                extract_exe_icon_to_png(&main_exe, Some(&game_dir), &icon_png_path);

            let mut cfg = DoorsGameConfig::new(
                plan.game_name.clone(),
                plan.slug.clone(),
                container_dir.clone(),
                main_exe.clone(),
                proton.name.clone(),
                proton.dir_path.clone(),
                if has_icon { Some(icon_png_path) } else { None },
            );
            cfg.desktop_shortcut = desktop_shortcut;
            cfg.umu = use_umu;
            if let Some(ref id) = umu_id {
                cfg.umu_id = id.clone();
                cfg.umu_store = initial_umu_store;
            }

            on_progress(0.97, tr("Creating application shortcut (.desktop)..."));
            save_config_and_create_desktop_entry(&cfg)?;

            if run_after_install {
                on_progress(1.0, tr("Completed! Launching game..."));
                let _ = spawn_with_proton_config(proton, &cfg, Some(&main_exe));
            } else {
                on_progress(1.0, tr("Game installation completed!"));
            }

            Ok(cfg)
        }

        InstallMode::SetupInstaller { installer_exe } => {
            let had_existing_config = container_dir.join("doors.json").exists();
            fs::create_dir_all(&game_dir)?;
            on_progress(
                0.25,
                tr("Running installer (Please complete the setup)..."),
            );

            // Run the setup wizard with Protonfixes disabled (`umu = false`) so game-specific
            // `replace_command` / CLI arg hooks never hijack `setup.exe` or `msiexec.exe`.
            let mut setup_cfg = DoorsGameConfig::new(
                plan.game_name.clone(),
                plan.slug.clone(),
                container_dir.clone(),
                installer_exe.clone(),
                proton.name.clone(),
                proton.dir_path.clone(),
                None,
            );
            setup_cfg.umu = false;

            let mut child = spawn_with_proton_config(proton, &setup_cfg, Some(installer_exe))?;
            let exit_status = child.wait();
            wait_for_prefix_wineserver(proton, &pfx_dir);

            on_progress(0.80, tr("Detecting installed main .exe..."));
            let (main_exe, discovered_title) =
                match detect_main_executable(&container_dir, None, &plan.game_name) {
                    Some(found) => found,
                    None => {
                        let pfx_initialized = pfx_dir.join("drive_c/windows").is_dir();
                        if !had_existing_config && is_dir_empty(&game_dir) {
                            let _ = fs::remove_dir_all(&container_dir);
                        }
                        if let Ok(st) = exit_status {
                            if !st.success() && !pfx_initialized {
                                let code_info = st
                                    .code()
                                    .map(|c| format!(" (code {c})"))
                                    .unwrap_or_default();
                                anyhow::bail!(
                                    "{} [{}{}]",
                                    tr("Proton failed to start the installer."),
                                    proton.name,
                                    code_info
                                );
                            }
                        }
                        anyhow::bail!(
                            "{}",
                            tr("No executable file (.exe) found in installation directory.")
                        );
                    }
                };

            let final_name = resolve_post_install_title(&plan.game_name, discovered_title, &main_exe);
            let final_slug = slugify(&final_name);

            on_progress(0.90, tr("Extracting icon (.ico -> .png)..."));
            let icon_png_path = container_dir.join("icon.png");
            let icon_source = resolve_installed_icon_source(
                &container_dir,
                &main_exe,
                Some(installer_exe),
                &final_name,
            );
            let has_icon =
                extract_exe_icon_to_png(&icon_source, main_exe.parent(), &icon_png_path);

            let mut cfg = DoorsGameConfig::new(
                final_name.clone(),
                final_slug,
                container_dir.clone(),
                main_exe.clone(),
                proton.name.clone(),
                proton.dir_path.clone(),
                if has_icon { Some(icon_png_path) } else { None },
            );
            cfg.desktop_shortcut = desktop_shortcut;
            apply_post_install_umu(
                &mut cfg,
                plan.umu_match.is_some(),
                use_umu,
                umu_id.as_deref(),
                &initial_umu_store,
            );

            let desired_folder = sanitize_folder_name(&final_name);
            if desired_folder != folder_name {
                let _ = cfg.rename_container(&desired_folder);
            }

            on_progress(0.96, tr("Creating application shortcut (.desktop)..."));
            save_config_and_create_desktop_entry(&cfg)?;

            if run_after_install {
                on_progress(1.0, tr("Completed! Launching game..."));
                let _ = spawn_with_proton_config(proton, &cfg, None);
            } else {
                on_progress(1.0, tr("Game installation completed!"));
            }
            Ok(cfg)
        }

        InstallMode::SingleExe { exe_path } => {
            let had_existing_config = container_dir.join("doors.json").exists();
            fs::create_dir_all(&game_dir)?;

            // Run the .exe inside the prefix first (in case it is an installer without "setup"/"install" in its name)
            on_progress(
                0.25,
                tr("Running installer (Please complete the setup)..."),
            );
            let mut setup_cfg = DoorsGameConfig::new(
                plan.game_name.clone(),
                plan.slug.clone(),
                container_dir.clone(),
                exe_path.clone(),
                proton.name.clone(),
                proton.dir_path.clone(),
                None,
            );
            setup_cfg.umu = false;

            let mut child = spawn_with_proton_config(proton, &setup_cfg, Some(exe_path))?;
            let exit_status = child.wait();
            wait_for_prefix_wineserver(proton, &pfx_dir);

            on_progress(0.80, tr("Detecting main .exe and icon..."));
            // Check if the executable installed a program into `pfx/drive_c`
            let (main_exe, discovered_title) = if let Some((installed_exe, title_opt)) =
                detect_main_executable(&container_dir, None, &plan.game_name)
            {
                (installed_exe, title_opt)
            } else {
                let pfx_initialized = pfx_dir.join("drive_c/windows").is_dir();
                if let Ok(st) = exit_status {
                    if !st.success() && !pfx_initialized {
                        if !had_existing_config && is_dir_empty(&game_dir) {
                            let _ = fs::remove_dir_all(&container_dir);
                        }
                        let code_info = st
                            .code()
                            .map(|c| format!(" (code {c})"))
                            .unwrap_or_default();
                        anyhow::bail!(
                            "{} [{}{}]",
                            tr("Proton failed to start the installer."),
                            proton.name,
                            code_info
                        );
                    }
                }
                // Nothing was installed into drive_c; it is a standalone portable .exe -> move it into game/
                let file_name = exe_path
                    .file_name()
                    .unwrap_or_else(|| std::ffi::OsStr::new("program.exe"));
                let dest_exe = game_dir.join(file_name);
                if exe_path != &dest_exe {
                    if fs::rename(exe_path, &dest_exe).is_err() {
                        fs::copy(exe_path, &dest_exe)?;
                        let _ = fs::remove_file(exe_path);
                    }
                }
                (dest_exe, None)
            };

            let final_name = resolve_post_install_title(&plan.game_name, discovered_title, &main_exe);
            let final_slug = slugify(&final_name);

            let icon_png_path = container_dir.join("icon.png");
            let icon_source = resolve_installed_icon_source(
                &container_dir,
                &main_exe,
                Some(exe_path),
                &final_name,
            );
            let has_icon =
                extract_exe_icon_to_png(&icon_source, main_exe.parent(), &icon_png_path);

            let mut cfg = DoorsGameConfig::new(
                final_name.clone(),
                final_slug,
                container_dir.clone(),
                main_exe.clone(),
                proton.name.clone(),
                proton.dir_path.clone(),
                if has_icon { Some(icon_png_path) } else { None },
            );
            cfg.desktop_shortcut = desktop_shortcut;
            apply_post_install_umu(
                &mut cfg,
                plan.umu_match.is_some(),
                use_umu,
                umu_id.as_deref(),
                &initial_umu_store,
            );

            let desired_folder = sanitize_folder_name(&final_name);
            if desired_folder != folder_name {
                let _ = cfg.rename_container(&desired_folder);
            }

            save_config_and_create_desktop_entry(&cfg)?;
            if run_after_install {
                on_progress(1.0, tr("Completed! Launching game..."));
                let _ = spawn_with_proton_config(proton, &cfg, None);
            } else {
                on_progress(1.0, tr("Game installation completed!"));
            }
            Ok(cfg)
        }
    }
}

fn resolve_post_install_title(
    initial_name: &str,
    discovered_title: Option<String>,
    main_exe: &Path,
) -> String {
    if !is_generic_metadata_boilerplate(initial_name) {
        return initial_name.to_string();
    }

    if let Some(title) = discovered_title {
        if !title.is_empty() && !is_generic_metadata_boilerplate(&title) {
            return title;
        }
    }

    if let Some(meta_title) = extract_exe_metadata(main_exe).best_title() {
        return meta_title;
    }

    main_exe
        .file_stem()
        .and_then(|s| s.to_str())
        .map(clean_game_title)
        .filter(|s| !s.is_empty() && !is_generic_metadata_boilerplate(s))
        .unwrap_or_else(|| initial_name.to_string())
}

fn apply_post_install_umu(
    cfg: &mut DoorsGameConfig,
    had_preinstall_match: bool,
    use_umu: bool,
    umu_id: Option<&str>,
    umu_store: &str,
) {
    if had_preinstall_match {
        cfg.umu = use_umu;
        if use_umu {
            if let Some(id) = umu_id {
                cfg.umu_id = id.to_string();
                cfg.umu_store = umu_store.to_string();
            }
        }
    } else if let Some(post_match) = crate::umu::lookup_umu_for_exe(&cfg.name, &cfg.main_exe) {
        // Pre-install setup binary was completely anonymous, so detect UMU from the installed executable
        cfg.umu = true;
        cfg.umu_id = post_match.umu_id;
        cfg.umu_store = post_match.store;
    }
}

fn is_dir_empty(dir: &Path) -> bool {
    match fs::read_dir(dir) {
        Ok(mut it) => it.next().is_none(),
        Err(_) => true,
    }
}

/// Move `src_dir` to `dst_dir`.
/// Uses instant `fs::rename` if on the same mount point; otherwise copies with byte-level progress and removes `src_dir`.
fn move_directory_with_progress<F>(
    src_dir: &Path,
    dst_dir: &Path,
    on_progress: &mut F,
) -> anyhow::Result<()>
where
    F: FnMut(f64, String),
{
    if let Some(parent) = dst_dir.parent() {
        fs::create_dir_all(parent)?;
    }

    if dst_dir.exists() {
        let _ = fs::remove_dir(dst_dir);
    }

    if fs::rename(src_dir, dst_dir).is_ok() {
        on_progress(0.80, tr("Folder moved."));
        return Ok(());
    }

    let mut total_bytes: u64 = 0;
    let mut files_to_copy: Vec<(PathBuf, PathBuf, u64)> = Vec::new();

    for entry in WalkDir::new(src_dir).into_iter().flatten() {
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(src_dir) else {
            continue;
        };
        let target = dst_dir.join(rel);
        if path.is_dir() {
            fs::create_dir_all(&target)?;
        } else if path.is_file() {
            let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
            total_bytes = total_bytes.saturating_add(len);
            files_to_copy.push((path.to_path_buf(), target, len));
        }
    }

    let mut copied_bytes: u64 = 0;
    let mut buf = vec![0u8; 4 * 1024 * 1024];
    let moving_label = tr("Moving...");

    for (src_file, dst_file, _len) in files_to_copy {
        if let Some(p) = dst_file.parent() {
            fs::create_dir_all(p)?;
        }
        let mut reader = File::open(&src_file)?;
        let mut writer = File::create(&dst_file)?;

        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            writer.write_all(&buf[..n])?;
            copied_bytes = copied_bytes.saturating_add(n as u64);

            if total_bytes > 0 {
                let ratio = (copied_bytes as f64 / total_bytes as f64).clamp(0.0, 1.0);
                let ui_progress = 0.05 + ratio * 0.75;
                let copied_gb = copied_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                let total_gb = total_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                on_progress(
                    ui_progress,
                    format!("{} {:.2} / {:.2} GB", moving_label, copied_gb, total_gb),
                );
            }
        }

        if let Ok(meta) = fs::metadata(&src_file) {
            let _ = fs::set_permissions(&dst_file, meta.permissions());
        }
    }

    let _ = fs::remove_dir_all(src_dir);
    Ok(())
}

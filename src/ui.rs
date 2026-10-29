use crate::desktop::{remove_desktop_shortcut, save_config_and_create_desktop_entry};
use crate::detector::{
    discover_installed_games, find_existing_doors_config, DoorsGameConfig, InstallPlan, SyncMode,
};
use crate::disks::discover_disks;
use crate::i18n::tr;
use crate::icon::{extract_exe_icon_png_bytes, extract_exe_icon_to_png};
use crate::installer::execute_install;
use crate::proton::{
    discover_proton_tools, has_host_binary, has_vkbasalt, spawn_winecfg, spawn_winetricks,
    spawn_with_proton_config, ProtonTool,
};
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

enum WorkerMessage {
    Progress(f64, String),
    Finished(Result<DoorsGameConfig, String>),
}

/// Entry point when an `.exe` file is double-clicked or passed via CLI.
pub fn handle_exe_launch(app: &adw::Application, exe_path: PathBuf) {
    if let Some((_cfg_path, cfg)) = find_existing_doors_config(&exe_path) {
        let tools = discover_proton_tools();
        let matched_tool = tools
            .iter()
            .find(|t| t.name == cfg.proton_name || t.dir_path == cfg.proton_dir)
            .cloned()
            .or_else(|| tools.into_iter().next());

        if let Some(tool) = matched_tool {
            let target_exe =
                if exe_path.is_file() && !crate::detector::is_auxiliary_exe(&exe_path) {
                    Some(exe_path.as_path())
                } else {
                    None
                };
            let _ = spawn_with_proton_config(&tool, &cfg, target_exe);
            return;
        }
    }

    present_install_window(app, exe_path);
}

/// Wizard-style minimal install window:
/// Shows extracted .exe icon + program title, Disk row with icon, Proton row with icon, and "Install" button.
pub fn present_install_window(app: &adw::Application, exe_path: PathBuf) {
    let plan = InstallPlan::analyze(&exe_path);
    let disks = discover_disks();
    let protons = discover_proton_tools();

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(&plan.game_name)
        .default_width(460)
        .default_height(380)
        .resizable(false)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let title_widget = adw::WindowTitle::new("Doors", "");
    header.set_title_widget(Some(&title_widget));

    let about_btn = gtk4::Button::from_icon_name("help-about-symbolic");
    about_btn.set_tooltip_text(Some(&tr("About")));
    header.pack_start(&about_btn);

    let win_for_about = window.clone();
    about_btn.connect_clicked(move |_| {
        present_about_dialog(&win_for_about);
    });

    toolbar_view.add_top_bar(&header);

    let main_box = gtk4::Box::new(gtk4::Orientation::Vertical, 20);
    main_box.set_margin_top(12);
    main_box.set_margin_bottom(24);
    main_box.set_margin_start(24);
    main_box.set_margin_end(24);

    // Wizard Hero Header: Extracted .exe icon + Title + .exe filename
    let hero_box = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    hero_box.set_halign(gtk4::Align::Center);

    let exe_icon = build_exe_preview_image(&exe_path, 68);
    exe_icon.add_css_class("icon-dropshadow");
    exe_icon.set_halign(gtk4::Align::Center);
    exe_icon.set_margin_bottom(4);
    hero_box.append(&exe_icon);

    let name_label = gtk4::Label::new(Some(&plan.game_name));
    name_label.add_css_class("title-2");
    name_label.set_wrap(true);
    name_label.set_justify(gtk4::Justification::Center);
    hero_box.append(&name_label);

    if let Some(exe_fname) = exe_path.file_name().and_then(|n| n.to_str()) {
        let sub_label = gtk4::Label::new(Some(exe_fname));
        sub_label.add_css_class("dim-label");
        sub_label.add_css_class("caption");
        hero_box.append(&sub_label);
    }

    main_box.append(&hero_box);

    let prefs_group = adw::PreferencesGroup::new();

    // 1. Disk selection row with harddisk icon
    let disk_strings: Vec<String> = if disks.is_empty() {
        vec![tr("No disks found")]
    } else {
        disks.iter().map(|d| d.display_row()).collect()
    };
    let disk_str_refs: Vec<&str> = disk_strings.iter().map(|s| s.as_str()).collect();
    let disk_model = gtk4::StringList::new(&disk_str_refs);
    let disk_row = adw::ComboRow::builder()
        .title(tr("Disk"))
        .model(&disk_model)
        .build();
    let disk_icon = gtk4::Image::from_icon_name("drive-harddisk-symbolic");
    disk_row.add_prefix(&disk_icon);

    if disks.len() > 1 {
        disk_row.set_selected(1);
    }

    // 2. Proton selection row with layer/addon icon
    let proton_strings: Vec<String> = if protons.is_empty() {
        vec![tr("compatibilitytools.d is empty")]
    } else {
        protons.iter().map(|p| p.name.clone()).collect()
    };
    let proton_str_refs: Vec<&str> = proton_strings.iter().map(|s| s.as_str()).collect();
    let proton_model = gtk4::StringList::new(&proton_str_refs);
    let proton_row = adw::ComboRow::builder()
        .title(tr("Proton"))
        .model(&proton_model)
        .build();
    let proton_icon = gtk4::Image::from_icon_name("application-x-addon-symbolic");
    proton_row.add_prefix(&proton_icon);

    // 3. "Run after installation" SwitchRow (off by default, switch on the right)
    let run_after_row = adw::SwitchRow::builder()
        .title(tr("Run after installation"))
        .active(false)
        .build();
    let run_icon = gtk4::Image::from_icon_name("media-playback-start-symbolic");
    run_after_row.add_prefix(&run_icon);

    prefs_group.add(&disk_row);
    prefs_group.add(&proton_row);
    prefs_group.add(&run_after_row);
    main_box.append(&prefs_group);

    // 4. "Install" button
    let install_btn = gtk4::Button::with_label(&tr("Install"));
    install_btn.add_css_class("suggested-action");
    install_btn.add_css_class("pill");
    install_btn.set_halign(gtk4::Align::Center);
    install_btn.set_width_request(190);

    if disks.is_empty() || protons.is_empty() {
        install_btn.set_sensitive(false);
    }

    main_box.append(&install_btn);

    // Progress section (hidden until "Install" is clicked)
    let progress_box = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    progress_box.set_visible(false);

    let progress_bar = gtk4::ProgressBar::new();
    progress_bar.set_show_text(true);

    let status_label = gtk4::Label::new(None);
    status_label.add_css_class("dim-label");
    status_label.set_wrap(true);

    progress_box.append(&progress_bar);
    progress_box.append(&status_label);
    main_box.append(&progress_box);

    toolbar_view.set_content(Some(&main_box));
    window.set_content(Some(&toolbar_view));

    let window_weak = window.downgrade();
    let disk_row_clone = disk_row.clone();
    let proton_row_clone = proton_row.clone();
    let run_after_clone = run_after_row.clone();
    let progress_box_clone = progress_box.clone();
    let progress_bar_clone = progress_bar.clone();
    let status_label_clone = status_label.clone();

    install_btn.connect_clicked(move |btn| {
        let disk_idx = disk_row_clone.selected() as usize;
        let proton_idx = proton_row_clone.selected() as usize;
        let run_after = run_after_clone.is_active();

        let Some(selected_disk) = disks.get(disk_idx).cloned() else {
            return;
        };
        let Some(selected_proton) = protons.get(proton_idx).cloned() else {
            return;
        };

        btn.set_visible(false);
        run_after_clone.set_sensitive(false);
        disk_row_clone.set_sensitive(false);
        proton_row_clone.set_sensitive(false);
        progress_box_clone.set_visible(true);
        progress_bar_clone.set_fraction(0.02);
        status_label_clone.set_text(&tr("Starting..."));

        let (tx, rx) = mpsc::channel::<WorkerMessage>();
        let plan_for_thread = plan.clone();

        std::thread::spawn(move || {
            let tx_progress = tx.clone();
            let result = execute_install(
                &plan_for_thread,
                &selected_disk,
                &selected_proton,
                run_after,
                move |fraction, msg| {
                    let _ = tx_progress.send(WorkerMessage::Progress(fraction, msg));
                },
            );
            match result {
                Ok(cfg) => {
                    let _ = tx.send(WorkerMessage::Finished(Ok(cfg)));
                }
                Err(err) => {
                    let _ = tx.send(WorkerMessage::Finished(Err(err.to_string())));
                }
            }
        });

        let progress_bar_ui = progress_bar_clone.clone();
        let status_label_ui = status_label_clone.clone();
        let btn_ui = btn.clone();
        let run_after_ui = run_after_clone.clone();
        let disk_row_ui = disk_row_clone.clone();
        let proton_row_ui = proton_row_clone.clone();
        let win_weak = window_weak.clone();

        glib::timeout_add_local(Duration::from_millis(50), move || {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    WorkerMessage::Progress(frac, text) => {
                        progress_bar_ui.set_fraction(frac.clamp(0.0, 1.0));
                        progress_bar_ui.set_text(Some(&format!("{:.0}%", frac * 100.0)));
                        status_label_ui.set_text(&text);
                    }
                    WorkerMessage::Finished(Ok(_cfg)) => {
                        progress_bar_ui.set_fraction(1.0);
                        status_label_ui.set_text(&tr("Completed!"));
                        if let Some(win) = win_weak.upgrade() {
                            win.close();
                        }
                        return glib::ControlFlow::Break;
                    }
                    WorkerMessage::Finished(Err(err_msg)) => {
                        status_label_ui.set_text(&format!("{} {}", tr("Error:"), err_msg));
                        btn_ui.set_visible(true);
                        run_after_ui.set_sensitive(true);
                        disk_row_ui.set_sensitive(true);
                        proton_row_ui.set_sensitive(true);
                        return glib::ControlFlow::Break;
                    }
                }
            }
            glib::ControlFlow::Continue
        });
    });

    window.present();
}

/// Extract `.exe` / `.msi` icon in-memory directly from the file itself and create a `gtk4::Image`
/// (with default fallback symbolic icon if the file has no embedded icon).
fn build_exe_preview_image(exe_path: &std::path::Path, size: i32) -> gtk4::Image {
    if let Some(png_bytes) = extract_exe_icon_png_bytes(exe_path, None) {
        let gbytes = glib::Bytes::from_owned(png_bytes);
        if let Ok(texture) = gdk4::Texture::from_bytes(&gbytes) {
            let img = gtk4::Image::from_paintable(Some(&texture));
            img.set_pixel_size(size);
            return img;
        }
    }
    let fallback = gtk4::Image::from_icon_name("application-x-executable-symbolic");
    fallback.set_pixel_size(size);
    fallback
}

/// Present the About dialog (matching zapret-gtk's AboutDialog design).
pub fn present_about_dialog(parent: &impl IsA<gtk4::Window>) {
    let icon_bytes = glib::Bytes::from_static(include_bytes!("../data/io.github.doors.Doors.png"));
    let logo_texture = gdk4::Texture::from_bytes(&icon_bytes).ok();

    let mut builder = gtk4::AboutDialog::builder()
        .transient_for(parent)
        .modal(true)
        .program_name("Doors")
        .version("0.2 Beta")
        .comments(tr(
            "Minimal GTK4/Libadwaita Windows (.exe) program installer and runner for Linux.",
        ))
        .website("https://github.com/Taygun86/Doors")
        .copyright("© 2026 Doors")
        .license_type(gtk4::License::Gpl30);

    if let Some(ref texture) = logo_texture {
        builder = builder.logo(texture);
    }

    let about = builder.build();
    about.present();
}

/// Library window when Doors is opened directly from the application menu.
pub fn present_library_window(app: &adw::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Doors")
        .default_width(640)
        .default_height(480)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let add_btn = gtk4::Button::from_icon_name("list-add-symbolic");
    add_btn.set_tooltip_text(Some(&tr("Install .exe")));
    header.pack_start(&add_btn);

    let about_btn = gtk4::Button::from_icon_name("help-about-symbolic");
    about_btn.set_tooltip_text(Some(&tr("About")));
    header.pack_end(&about_btn);

    let win_for_about = window.clone();
    about_btn.connect_clicked(move |_| {
        present_about_dialog(&win_for_about);
    });

    toolbar_view.add_top_bar(&header);

    let content_holder = Rc::new(RefCell::new(gtk4::Box::new(
        gtk4::Orientation::Vertical,
        0,
    )));
    toolbar_view.set_content(Some(&*content_holder.borrow()));
    window.set_content(Some(&toolbar_view));

    let app_clone = app.clone();
    let window_clone = window.clone();
    add_btn.connect_clicked(move |_| {
        let dialog = gtk4::FileDialog::builder()
            .title(tr("Select Executable (.exe)"))
            .modal(true)
            .build();

        let filter = gtk4::FileFilter::new();
        filter.set_name(Some(&tr("Windows Executable (*.exe)")));
        filter.add_pattern("*.exe");
        filter.add_pattern("*.EXE");
        filter.add_pattern("*.msi");
        filter.add_pattern("*.MSI");
        let filters = gio::ListStore::new::<gtk4::FileFilter>();
        filters.append(&filter);
        dialog.set_filters(Some(&filters));

        let app_inner = app_clone.clone();
        dialog.open(
            Some(&window_clone),
            gio::Cancellable::NONE,
            move |res| {
                if let Ok(file) = res {
                    if let Some(path) = file.path() {
                        present_install_window(&app_inner, path);
                    }
                }
            },
        );
    });

    rebuild_library_list(&window, &content_holder.borrow());
    window.present();
}

fn rebuild_library_list(parent_win: &adw::ApplicationWindow, container: &gtk4::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let disks = discover_disks();
    let protons = discover_proton_tools();
    let programs = discover_installed_games(&disks);

    if programs.is_empty() {
        let status = adw::StatusPage::builder()
            .icon_name("application-x-executable-symbolic")
            .title(tr("No Programs Yet"))
            .description(tr("Double-click any .exe file or use the + button to install."))
            .vexpand(true)
            .build();
        container.append(&status);
        return;
    }

    let scrolled = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .build();

    let clamp = adw::Clamp::builder()
        .maximum_size(600)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();

    let group = adw::PreferencesGroup::builder()
        .title(tr("Installed Programs"))
        .build();

    for prog in programs {
        let subtitle = format!(
            "{} • {}",
            prog.proton_name,
            prog.container_dir.display()
        );
        let row = adw::ActionRow::builder()
            .title(&prog.name)
            .subtitle(&subtitle)
            .build();

        // Program icon
        let img = if let Some(ref icon_path) = prog.icon_png {
            if icon_path.is_file() {
                gtk4::Image::from_file(icon_path)
            } else {
                gtk4::Image::from_icon_name("application-x-executable-symbolic")
            }
        } else {
            gtk4::Image::from_icon_name("application-x-executable-symbolic")
        };
        img.set_pixel_size(40);
        row.add_prefix(&img);

        // Run button
        let run_btn = gtk4::Button::with_label(&tr("Run"));
        run_btn.add_css_class("suggested-action");
        run_btn.set_valign(gtk4::Align::Center);

        let prog_run = prog.clone();
        let protons_run: Vec<ProtonTool> = protons.clone();
        run_btn.connect_clicked(move |_| {
            let cfg_path = prog_run.container_dir.join("doors.json");
            let latest_cfg = fs::read_to_string(&cfg_path)
                .ok()
                .and_then(|s| serde_json::from_str::<DoorsGameConfig>(&s).ok())
                .unwrap_or_else(|| prog_run.clone());

            let tool = protons_run
                .iter()
                .find(|t| t.name == latest_cfg.proton_name || t.dir_path == latest_cfg.proton_dir)
                .cloned()
                .or_else(|| protons_run.first().cloned());

            if let Some(t) = tool {
                let _ = spawn_with_proton_config(&t, &latest_cfg, None);
            }
        });

        // Settings gear button
        let settings_btn = gtk4::Button::from_icon_name("emblem-system-symbolic");
        settings_btn.add_css_class("flat");
        settings_btn.set_valign(gtk4::Align::Center);
        settings_btn.set_tooltip_text(Some(&tr("Settings")));

        let prog_settings = prog.clone();
        let parent_win_clone = parent_win.clone();
        let container_for_settings = container.clone();
        settings_btn.connect_clicked(move |_| {
            let p_win = parent_win_clone.clone();
            let c_box = container_for_settings.clone();
            present_program_settings_window(
                &parent_win_clone,
                prog_settings.clone(),
                Rc::new(move || {
                    rebuild_library_list(&p_win, &c_box);
                }),
            );
        });

        // Delete button
        let del_btn = gtk4::Button::from_icon_name("user-trash-symbolic");
        del_btn.add_css_class("flat");
        del_btn.set_valign(gtk4::Align::Center);
        del_btn.set_tooltip_text(Some(&tr("Delete Program and Prefix")));

        let prog_del = prog.clone();
        let parent_win_del = parent_win.clone();
        let container_clone = container.clone();
        del_btn.connect_clicked(move |_| {
            remove_desktop_shortcut(&prog_del);
            let _ = fs::remove_dir_all(&prog_del.container_dir);
            rebuild_library_list(&parent_win_del, &container_clone);
        });

        row.add_suffix(&run_btn);
        row.add_suffix(&settings_btn);
        row.add_suffix(&del_btn);
        group.add(&row);
    }

    clamp.set_child(Some(&group));
    scrolled.set_child(Some(&clamp));
    container.append(&scrolled);
}

/// Per-program Settings Window opened via the gear button.
fn present_program_settings_window(
    parent: &adw::ApplicationWindow,
    initial_cfg: DoorsGameConfig,
    on_saved: Rc<dyn Fn()>,
) {
    let cfg_file = initial_cfg.container_dir.join("doors.json");
    let loaded_cfg = fs::read_to_string(&cfg_file)
        .ok()
        .and_then(|s| serde_json::from_str::<DoorsGameConfig>(&s).ok())
        .unwrap_or(initial_cfg);

    let cfg_state = Rc::new(RefCell::new(loaded_cfg.clone()));
    let protons = discover_proton_tools();

    let win = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title(format!("{} — {}", loaded_cfg.name, tr("Settings")))
        .default_width(560)
        .default_height(640)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let scrolled = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .build();

    let clamp = adw::Clamp::builder()
        .maximum_size(520)
        .margin_top(16)
        .margin_bottom(24)
        .margin_start(16)
        .margin_end(16)
        .build();

    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 20);

    // --- Group 1: Runner & Launch ---
    let runner_group = adw::PreferencesGroup::builder()
        .title(tr("Runner & Launch"))
        .build();

    // Program Name EntryRow
    let name_row = adw::EntryRow::builder()
        .title(tr("Program Name"))
        .text(&loaded_cfg.name)
        .show_apply_button(true)
        .build();
    let name_icon = gtk4::Image::from_icon_name("document-edit-symbolic");
    name_row.add_prefix(&name_icon);

    // Proton selector
    let proton_strings: Vec<String> = if protons.is_empty() {
        vec![loaded_cfg.proton_name.clone()]
    } else {
        protons.iter().map(|p| p.name.clone()).collect()
    };
    let proton_refs: Vec<&str> = proton_strings.iter().map(|s| s.as_str()).collect();
    let proton_model = gtk4::StringList::new(&proton_refs);
    let proton_row = adw::ComboRow::builder()
        .title(tr("Proton"))
        .model(&proton_model)
        .build();
    let proton_icon = gtk4::Image::from_icon_name("application-x-addon-symbolic");
    proton_row.add_prefix(&proton_icon);

    if let Some(idx) = protons
        .iter()
        .position(|p| p.name == loaded_cfg.proton_name || p.dir_path == loaded_cfg.proton_dir)
    {
        proton_row.set_selected(idx as u32);
    }

    // Main Executable (.exe) Selector Row
    let exe_subtitle = loaded_cfg
        .main_exe
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("program.exe")
        .to_string();
    let main_exe_row = adw::ActionRow::builder()
        .title(tr("Main Executable (.exe)"))
        .subtitle(&exe_subtitle)
        .build();
    let main_exe_icon = gtk4::Image::from_icon_name("application-x-executable-symbolic");
    main_exe_row.add_prefix(&main_exe_icon);

    let change_exe_btn = gtk4::Button::with_label(&tr("Change"));
    change_exe_btn.set_valign(gtk4::Align::Center);
    main_exe_row.add_suffix(&change_exe_btn);

    // Launch Arguments EntryRow
    let args_row = adw::EntryRow::builder()
        .title(tr("Launch Arguments"))
        .text(&loaded_cfg.launch_args)
        .build();
    let args_icon = gtk4::Image::from_icon_name("utilities-terminal-symbolic");
    args_row.add_prefix(&args_icon);

    // Synchronization Mode ComboRow
    let sync_labels = ["FSync", "NTSync", "ESync", &tr("Disabled")];
    let sync_model = gtk4::StringList::new(&sync_labels);
    let sync_row = adw::ComboRow::builder()
        .title(tr("Synchronization"))
        .subtitle(tr("Select Wine/Proton synchronization method."))
        .model(&sync_model)
        .build();
    let sync_idx = match loaded_cfg.sync_mode {
        SyncMode::Fsync => 0,
        SyncMode::Ntsync => 1,
        SyncMode::Esync => 2,
        SyncMode::None => 3,
    };
    sync_row.set_selected(sync_idx);

    runner_group.add(&name_row);
    runner_group.add(&proton_row);
    runner_group.add(&main_exe_row);
    runner_group.add(&args_row);
    runner_group.add(&sync_row);
    content_box.append(&runner_group);

    // --- Group 2: Display & Performance ---
    let perf_group = adw::PreferencesGroup::builder()
        .title(tr("Display & Performance"))
        .build();

    // Native Wayland SwitchRow
    let wayland_row = adw::SwitchRow::builder()
        .title(tr("Native Wayland"))
        .subtitle(tr("Force running directly on Wayland instead of Xwayland."))
        .active(loaded_cfg.wayland)
        .build();

    // High Dynamic Range (HDR) SwitchRow
    let hdr_row = adw::SwitchRow::builder()
        .title(tr("High Dynamic Range"))
        .subtitle(tr("Expose HDR using DXVK. Requires native Wayland or Gamescope. Older NVIDIA drivers also need HdrWsi and ENABLE_HDR_WSI=1."))
        .active(loaded_cfg.hdr)
        .build();

    // Discrete GPU SwitchRow
    let dgpu_row = adw::SwitchRow::builder()
        .title(tr("Use Discrete GPU"))
        .subtitle(tr("Prefer dedicated NVIDIA/AMD graphics card over integrated GPU."))
        .active(loaded_cfg.discrete_gpu)
        .build();

    // NVIDIA DLSS / NVAPI SwitchRow
    let nvapi_row = adw::SwitchRow::builder()
        .title(tr("NVIDIA DLSS / NVAPI"))
        .subtitle(tr("Enable NVAPI, DLSS, and Reflex support (PROTON_ENABLE_NVAPI=1)."))
        .active(loaded_cfg.nvapi)
        .build();

    // AMD FSR SwitchRow
    let fsr_row = adw::SwitchRow::builder()
        .title(tr("AMD FidelityFX Super Resolution (FSR)"))
        .subtitle(tr("Enable fullscreen FSR upscaling in supported Proton builds."))
        .active(loaded_cfg.fsr)
        .build();

    // vkBasalt row
    let vkbasalt_installed = has_vkbasalt();
    let vkbasalt_row = adw::ActionRow::builder()
        .title("vkBasalt")
        .subtitle(tr("Enable Vulkan post-processing and sharpening layer."))
        .build();
    let vkbasalt_switch = gtk4::Switch::builder()
        .valign(gtk4::Align::Center)
        .active(loaded_cfg.vkbasalt && vkbasalt_installed)
        .sensitive(vkbasalt_installed)
        .build();
    vkbasalt_row.add_suffix(&vkbasalt_switch);
    vkbasalt_row.set_activatable_widget(Some(&vkbasalt_switch));
    if !vkbasalt_installed {
        let warn_icon = gtk4::Image::from_icon_name("dialog-warning-symbolic");
        warn_icon.set_tooltip_text(Some(&tr("vkBasalt is not installed on the system")));
        vkbasalt_row.add_suffix(&warn_icon);
    }

    // GameMode row (auto-enabled if installed on system)
    let gamemode_installed = has_host_binary("gamemoderun");
    let gamemode_row = adw::ActionRow::builder()
        .title("GameMode")
        .subtitle(tr("Optimize system performance on demand using Feral GameMode."))
        .build();
    let gamemode_switch = gtk4::Switch::builder()
        .valign(gtk4::Align::Center)
        .active(loaded_cfg.gamemode && gamemode_installed)
        .sensitive(gamemode_installed)
        .build();
    gamemode_row.add_suffix(&gamemode_switch);
    gamemode_row.set_activatable_widget(Some(&gamemode_switch));
    if !gamemode_installed {
        let warn_icon = gtk4::Image::from_icon_name("dialog-warning-symbolic");
        warn_icon.set_tooltip_text(Some(&tr("gamemode is not installed on the system")));
        gamemode_row.add_suffix(&warn_icon);
    }

    // MangoHud row (disabled by default even if installed)
    let mangohud_installed = has_host_binary("mangohud");
    let mangohud_row = adw::ActionRow::builder()
        .title("MangoHud")
        .subtitle(tr("Display FPS, temperatures, and frame timing overlay."))
        .build();
    let mangohud_switch = gtk4::Switch::builder()
        .valign(gtk4::Align::Center)
        .active(loaded_cfg.mangohud && mangohud_installed)
        .sensitive(mangohud_installed)
        .build();
    mangohud_row.add_suffix(&mangohud_switch);
    mangohud_row.set_activatable_widget(Some(&mangohud_switch));
    if !mangohud_installed {
        let warn_icon = gtk4::Image::from_icon_name("dialog-warning-symbolic");
        warn_icon.set_tooltip_text(Some(&tr("mangohud is not installed on the system")));
        mangohud_row.add_suffix(&warn_icon);
    }

    // Gamescope row
    let gamescope_installed = has_host_binary("gamescope");
    let gamescope_row = adw::ActionRow::builder()
        .title("Gamescope")
        .subtitle(tr("Manage how programs are displayed on screen using Gamescope."))
        .build();

    let gs_gear_btn = gtk4::MenuButton::builder()
        .icon_name("emblem-system-symbolic")
        .valign(gtk4::Align::Center)
        .sensitive(gamescope_installed)
        .build();
    gs_gear_btn.add_css_class("flat");

    let gs_popover = build_gamescope_popover(cfg_state.clone(), on_saved.clone());
    gs_gear_btn.set_popover(Some(&gs_popover));

    let gamescope_switch = gtk4::Switch::builder()
        .valign(gtk4::Align::Center)
        .active(loaded_cfg.gamescope && gamescope_installed)
        .sensitive(gamescope_installed)
        .build();

    gamescope_row.add_suffix(&gs_gear_btn);
    gamescope_row.add_suffix(&gamescope_switch);
    gamescope_row.set_activatable_widget(Some(&gamescope_switch));
    if !gamescope_installed {
        let warn_icon = gtk4::Image::from_icon_name("dialog-warning-symbolic");
        warn_icon.set_tooltip_text(Some(&tr("gamescope is not installed on the system")));
        gamescope_row.add_suffix(&warn_icon);
    }

    perf_group.add(&wayland_row);
    perf_group.add(&hdr_row);
    perf_group.add(&dgpu_row);
    perf_group.add(&nvapi_row);
    perf_group.add(&fsr_row);
    perf_group.add(&vkbasalt_row);
    perf_group.add(&gamemode_row);
    perf_group.add(&mangohud_row);
    perf_group.add(&gamescope_row);
    content_box.append(&perf_group);

    // --- Group 3: Prefix & Tools ---
    let tools_group = adw::PreferencesGroup::builder()
        .title(tr("Prefix & Tools"))
        .build();

    // Open Program Folder
    let open_folder_row = adw::ActionRow::builder()
        .title(tr("Open Program Folder"))
        .subtitle(tr("Browse program files and prefix directory in file manager."))
        .build();
    let open_folder_btn = gtk4::Button::with_label(&tr("Open"));
    open_folder_btn.set_valign(gtk4::Align::Center);
    open_folder_row.add_suffix(&open_folder_btn);

    // Run .exe in Prefix (for patches, updates, redistributables)
    let run_in_pfx_row = adw::ActionRow::builder()
        .title(tr("Run .exe in Prefix"))
        .subtitle(tr("Run a patch, update, or redistributable installer inside this prefix."))
        .build();
    let run_in_pfx_btn = gtk4::Button::with_label(&tr("Select .exe"));
    run_in_pfx_btn.set_valign(gtk4::Align::Center);
    run_in_pfx_row.add_suffix(&run_in_pfx_btn);

    // Wine Tools (winecfg & Winetricks)
    let wine_tools_row = adw::ActionRow::builder()
        .title(tr("Wine Tools"))
        .subtitle(tr("Open Wine configuration (winecfg) or Winetricks for this prefix."))
        .build();
    let winecfg_btn = gtk4::Button::with_label("winecfg");
    winecfg_btn.set_valign(gtk4::Align::Center);
    wine_tools_row.add_suffix(&winecfg_btn);

    let winetricks_installed = has_host_binary("winetricks");
    let winetricks_btn = gtk4::Button::with_label("Winetricks");
    winetricks_btn.set_valign(gtk4::Align::Center);
    winetricks_btn.set_sensitive(winetricks_installed);
    wine_tools_row.add_suffix(&winetricks_btn);

    // Reset Prefix
    let reset_pfx_row = adw::ActionRow::builder()
        .title(tr("Reset Prefix"))
        .subtitle(tr("Delete and recreate the Wine/Proton prefix (pfx) without touching program files."))
        .build();
    let reset_pfx_btn = gtk4::Button::with_label(&tr("Reset"));
    reset_pfx_btn.add_css_class("destructive-action");
    reset_pfx_btn.set_valign(gtk4::Align::Center);
    reset_pfx_row.add_suffix(&reset_pfx_btn);

    tools_group.add(&open_folder_row);
    tools_group.add(&run_in_pfx_row);
    tools_group.add(&wine_tools_row);
    tools_group.add(&reset_pfx_row);
    content_box.append(&tools_group);

    clamp.set_child(Some(&content_box));
    scrolled.set_child(Some(&clamp));
    toolbar_view.set_content(Some(&scrolled));
    win.set_content(Some(&toolbar_view));

    // Helper closure to save config
    let save_now = {
        let cfg_state = cfg_state.clone();
        let on_saved = on_saved.clone();
        Rc::new(move || {
            let cfg = cfg_state.borrow().clone();
            let _ = save_config_and_create_desktop_entry(&cfg);
            on_saved();
        })
    };

    // Change Main .exe button handler
    {
        let cfg_state = cfg_state.clone();
        let save_now = save_now.clone();
        let main_exe_row_clone = main_exe_row.clone();
        let win_clone = win.clone();
        change_exe_btn.connect_clicked(move |_| {
            let dialog = gtk4::FileDialog::builder()
                .title(tr("Select Executable (.exe)"))
                .modal(true)
                .build();

            let start_dir = cfg_state.borrow().container_dir.clone();
            if start_dir.is_dir() {
                dialog.set_initial_folder(Some(&gio::File::for_path(&start_dir)));
            }

            let filter = gtk4::FileFilter::new();
            filter.set_name(Some(&tr("Windows Executable (*.exe)")));
            filter.add_pattern("*.exe");
            filter.add_pattern("*.EXE");
            filter.add_pattern("*.msi");
            filter.add_pattern("*.MSI");
            let filters = gio::ListStore::new::<gtk4::FileFilter>();
            filters.append(&filter);
            dialog.set_filters(Some(&filters));

            let cfg_inner = cfg_state.clone();
            let save_inner = save_now.clone();
            let row_inner = main_exe_row_clone.clone();
            dialog.open(Some(&win_clone), gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res {
                    if let Some(new_exe) = file.path() {
                        let mut cfg = cfg_inner.borrow_mut();
                        cfg.main_exe = new_exe.clone();
                        let icon_png_path = cfg.container_dir.join("icon.png");
                        if extract_exe_icon_to_png(&new_exe, new_exe.parent(), &icon_png_path) {
                            cfg.icon_png = Some(icon_png_path);
                        }
                        if let Some(fname) = new_exe.file_name().and_then(|s| s.to_str()) {
                            row_inner.set_subtitle(fname);
                        }
                        drop(cfg);
                        save_inner();
                    }
                }
            });
        });
    }

    // Open Program Folder handler
    {
        let cfg_state = cfg_state.clone();
        open_folder_btn.connect_clicked(move |_| {
            let folder = cfg_state.borrow().container_dir.clone();
            let _ = Command::new("xdg-open").arg(&folder).spawn();
        });
    }

    // Run .exe in Prefix handler
    {
        let cfg_state = cfg_state.clone();
        let protons_clone = protons.clone();
        let win_clone = win.clone();
        run_in_pfx_btn.connect_clicked(move |_| {
            let dialog = gtk4::FileDialog::builder()
                .title(tr("Select Executable (.exe)"))
                .modal(true)
                .build();

            let filter = gtk4::FileFilter::new();
            filter.set_name(Some(&tr("Windows Executable (*.exe)")));
            filter.add_pattern("*.exe");
            filter.add_pattern("*.EXE");
            filter.add_pattern("*.msi");
            filter.add_pattern("*.MSI");
            let filters = gio::ListStore::new::<gtk4::FileFilter>();
            filters.append(&filter);
            dialog.set_filters(Some(&filters));

            let cfg_inner = cfg_state.clone();
            let protons_inner = protons_clone.clone();
            dialog.open(Some(&win_clone), gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res {
                    if let Some(chosen_exe) = file.path() {
                        let cfg = cfg_inner.borrow().clone();
                        let tool = protons_inner
                            .iter()
                            .find(|t| t.name == cfg.proton_name || t.dir_path == cfg.proton_dir)
                            .cloned()
                            .or_else(|| protons_inner.first().cloned());
                        if let Some(t) = tool {
                            let _ = spawn_with_proton_config(&t, &cfg, Some(&chosen_exe));
                        }
                    }
                }
            });
        });
    }

    // winecfg button handler
    {
        let cfg_state = cfg_state.clone();
        let protons_clone = protons.clone();
        winecfg_btn.connect_clicked(move |_| {
            let cfg = cfg_state.borrow().clone();
            let tool = protons_clone
                .iter()
                .find(|t| t.name == cfg.proton_name || t.dir_path == cfg.proton_dir)
                .cloned()
                .or_else(|| protons_clone.first().cloned());
            if let Some(t) = tool {
                let _ = spawn_winecfg(&t, &cfg);
            }
        });
    }

    // Winetricks button handler
    {
        let cfg_state = cfg_state.clone();
        let protons_clone = protons.clone();
        winetricks_btn.connect_clicked(move |_| {
            let cfg = cfg_state.borrow().clone();
            let tool = protons_clone
                .iter()
                .find(|t| t.name == cfg.proton_name || t.dir_path == cfg.proton_dir)
                .cloned()
                .or_else(|| protons_clone.first().cloned());
            if let Some(t) = tool {
                let _ = spawn_winetricks(&t, &cfg);
            }
        });
    }

    // Reset Prefix button handler
    {
        let cfg_state = cfg_state.clone();
        reset_pfx_btn.connect_clicked(move |btn| {
            let pfx_dir = cfg_state.borrow().container_dir.join("pfx");
            let _ = fs::remove_dir_all(&pfx_dir);
            let _ = fs::create_dir_all(&pfx_dir);
            btn.set_label(&tr("Prefix Reset!"));
            btn.set_sensitive(false);
        });
    }

    // Live saving handlers for all switches/combos
    {
        let cfg_state = cfg_state.clone();
        let protons_clone = protons.clone();
        let save_now = save_now.clone();
        proton_row.connect_selected_notify(move |row| {
            let idx = row.selected() as usize;
            if let Some(tool) = protons_clone.get(idx) {
                let mut cfg = cfg_state.borrow_mut();
                cfg.proton_name = tool.name.clone();
                cfg.proton_dir = tool.dir_path.clone();
                drop(cfg);
                save_now();
            }
        });
    }

    {
        let cfg_state = cfg_state.clone();
        let save_now = save_now.clone();
        args_row.connect_changed(move |entry| {
            cfg_state.borrow_mut().launch_args = entry.text().to_string();
            save_now();
        });
    }

    {
        let cfg_state = cfg_state.clone();
        let save_now = save_now.clone();
        sync_row.connect_selected_notify(move |row| {
            let mode = match row.selected() {
                0 => SyncMode::Fsync,
                1 => SyncMode::Ntsync,
                2 => SyncMode::Esync,
                _ => SyncMode::None,
            };
            cfg_state.borrow_mut().sync_mode = mode;
            save_now();
        });
    }

    {
        let cfg_state = cfg_state.clone();
        let save_now = save_now.clone();
        wayland_row.connect_active_notify(move |row| {
            cfg_state.borrow_mut().wayland = row.is_active();
            save_now();
        });
    }

    {
        let cfg_state = cfg_state.clone();
        let save_now = save_now.clone();
        hdr_row.connect_active_notify(move |row| {
            cfg_state.borrow_mut().hdr = row.is_active();
            save_now();
        });
    }

    {
        let cfg_state = cfg_state.clone();
        let save_now = save_now.clone();
        dgpu_row.connect_active_notify(move |row| {
            cfg_state.borrow_mut().discrete_gpu = row.is_active();
            save_now();
        });
    }

    {
        let cfg_state = cfg_state.clone();
        let save_now = save_now.clone();
        nvapi_row.connect_active_notify(move |row| {
            cfg_state.borrow_mut().nvapi = row.is_active();
            save_now();
        });
    }

    {
        let cfg_state = cfg_state.clone();
        let save_now = save_now.clone();
        fsr_row.connect_active_notify(move |row| {
            cfg_state.borrow_mut().fsr = row.is_active();
            save_now();
        });
    }

    {
        let cfg_state = cfg_state.clone();
        let save_now = save_now.clone();
        vkbasalt_switch.connect_state_set(move |_, state| {
            cfg_state.borrow_mut().vkbasalt = state;
            save_now();
            glib::Propagation::Proceed
        });
    }

    {
        let cfg_state = cfg_state.clone();
        let save_now = save_now.clone();
        gamemode_switch.connect_state_set(move |_, state| {
            cfg_state.borrow_mut().gamemode = state;
            save_now();
            glib::Propagation::Proceed
        });
    }

    {
        let cfg_state = cfg_state.clone();
        let save_now = save_now.clone();
        mangohud_switch.connect_state_set(move |_, state| {
            cfg_state.borrow_mut().mangohud = state;
            save_now();
            glib::Propagation::Proceed
        });
    }

    {
        let cfg_state = cfg_state.clone();
        let save_now = save_now.clone();
        gamescope_switch.connect_state_set(move |_, state| {
            cfg_state.borrow_mut().gamescope = state;
            save_now();
            glib::Propagation::Proceed
        });
    }

    // Program Name update handler (updates doors.json + .desktop shortcut + window/library title)
    let apply_name_change: Rc<dyn Fn(&str)> = {
        let cfg_state = cfg_state.clone();
        let save_now = save_now.clone();
        let win_weak = win.downgrade();
        Rc::new(move |raw_text: &str| {
            let trimmed = raw_text.trim();
            if trimmed.is_empty() {
                return;
            }
            let mut cfg = cfg_state.borrow_mut();
            if cfg.name != trimmed {
                remove_desktop_shortcut(&cfg);
                cfg.name = trimmed.to_string();
                cfg.slug = crate::detector::slugify(trimmed);
                if let Some(w) = win_weak.upgrade() {
                    w.set_title(Some(&format!("{} — {}", cfg.name, tr("Settings"))));
                }
                drop(cfg);
                save_now();
            }
        })
    };

    {
        let apply_name = apply_name_change.clone();
        name_row.connect_apply(move |entry| {
            apply_name(&entry.text());
        });
    }

    {
        let apply_name = apply_name_change.clone();
        let name_row_close = name_row.clone();
        win.connect_close_request(move |_| {
            apply_name(&name_row_close.text());
            glib::Propagation::Proceed
        });
    }

    win.present();
}

/// Popover attached to the Gamescope gear icon (`⚙`) for resolution, FPS limit, and fullscreen.
fn build_gamescope_popover(
    cfg_state: Rc<RefCell<DoorsGameConfig>>,
    on_saved: Rc<dyn Fn()>,
) -> gtk4::Popover {
    let popover = gtk4::Popover::new();
    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);

    let cfg_init = cfg_state.borrow().clone();

    let w_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let w_lbl = gtk4::Label::new(Some(&tr("Width")));
    w_lbl.set_hexpand(true);
    w_lbl.set_xalign(0.0);
    let w_entry = gtk4::Entry::builder()
        .width_chars(7)
        .placeholder_text("1920")
        .text(if cfg_init.gamescope_width > 0 {
            cfg_init.gamescope_width.to_string()
        } else {
            String::new()
        })
        .build();
    w_box.append(&w_lbl);
    w_box.append(&w_entry);

    let h_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let h_lbl = gtk4::Label::new(Some(&tr("Height")));
    h_lbl.set_hexpand(true);
    h_lbl.set_xalign(0.0);
    let h_entry = gtk4::Entry::builder()
        .width_chars(7)
        .placeholder_text("1080")
        .text(if cfg_init.gamescope_height > 0 {
            cfg_init.gamescope_height.to_string()
        } else {
            String::new()
        })
        .build();
    h_box.append(&h_lbl);
    h_box.append(&h_entry);

    let fps_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let fps_lbl = gtk4::Label::new(Some(&tr("FPS Limit")));
    fps_lbl.set_hexpand(true);
    fps_lbl.set_xalign(0.0);
    let fps_entry = gtk4::Entry::builder()
        .width_chars(7)
        .placeholder_text("60")
        .text(if cfg_init.gamescope_fps > 0 {
            cfg_init.gamescope_fps.to_string()
        } else {
            String::new()
        })
        .build();
    fps_box.append(&fps_lbl);
    fps_box.append(&fps_entry);

    let fs_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let fs_lbl = gtk4::Label::new(Some(&tr("Fullscreen")));
    fs_lbl.set_hexpand(true);
    fs_lbl.set_xalign(0.0);
    let fs_switch = gtk4::Switch::builder()
        .active(cfg_init.gamescope_fullscreen)
        .valign(gtk4::Align::Center)
        .build();
    fs_box.append(&fs_lbl);
    fs_box.append(&fs_switch);

    vbox.append(&w_box);
    vbox.append(&h_box);
    vbox.append(&fps_box);
    vbox.append(&fs_box);
    popover.set_child(Some(&vbox));

    let save_gs = {
        let cfg_state = cfg_state.clone();
        let on_saved = on_saved.clone();
        let w_entry = w_entry.clone();
        let h_entry = h_entry.clone();
        let fps_entry = fps_entry.clone();
        let fs_switch = fs_switch.clone();
        Rc::new(move || {
            let mut cfg = cfg_state.borrow_mut();
            cfg.gamescope_width = w_entry.text().trim().parse::<u32>().unwrap_or(0);
            cfg.gamescope_height = h_entry.text().trim().parse::<u32>().unwrap_or(0);
            cfg.gamescope_fps = fps_entry.text().trim().parse::<u32>().unwrap_or(0);
            cfg.gamescope_fullscreen = fs_switch.is_active();
            let cloned = cfg.clone();
            drop(cfg);
            let _ = save_config_and_create_desktop_entry(&cloned);
            on_saved();
        })
    };

    let s1 = save_gs.clone();
    w_entry.connect_changed(move |_| s1());
    let s2 = save_gs.clone();
    h_entry.connect_changed(move |_| s2());
    let s3 = save_gs.clone();
    fps_entry.connect_changed(move |_| s3());
    let s4 = save_gs.clone();
    fs_switch.connect_state_set(move |_, _| {
        s4();
        glib::Propagation::Proceed
    });

    popover
}

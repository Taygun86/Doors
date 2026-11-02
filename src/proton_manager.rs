use crate::detector::discover_installed_games;
use crate::disks::{discover_disks, real_user_home};
use crate::i18n::tr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::Duration;

static BG_UPDATE_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedProton {
    GeProton,
    DwProton,
    ProtonCachyos,
    ProtonRtsp,
}

#[derive(Debug, Clone)]
pub struct ProtonReleaseInfo {
    pub tag_name: String,
    pub asset_url: String,
    pub asset_name: String,
    pub size: u64,
}

impl ManagedProton {
    pub const ALL: [ManagedProton; 4] = [
        ManagedProton::GeProton,
        ManagedProton::DwProton,
        ManagedProton::ProtonCachyos,
        ManagedProton::ProtonRtsp,
    ];

    #[allow(dead_code)]
    pub fn id(&self) -> &'static str {
        match self {
            ManagedProton::GeProton => "ge-proton",
            ManagedProton::DwProton => "dw-proton",
            ManagedProton::ProtonCachyos => "proton-cachyos",
            ManagedProton::ProtonRtsp => "proton-rtsp",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            ManagedProton::GeProton => "Proton-GE",
            ManagedProton::DwProton => "DW-Proton",
            ManagedProton::ProtonCachyos => "Proton-CachyOS",
            ManagedProton::ProtonRtsp => "Proton-RTSP",
        }
    }

    pub fn description(&self) -> String {
        match self {
            ManagedProton::GeProton => tr("Community optimized Wine/Proton with media codec patches."),
            ManagedProton::DwProton => tr("Tailored for anime and gacha games with DXVK-gplasync."),
            ManagedProton::ProtonCachyos => tr("CachyOS bleeding-edge performance and Wine core optimizations."),
            ManagedProton::ProtonRtsp => tr("Live video streaming (RTSP/MPEG-TS) support, tailored for VRChat and media players."),
        }
    }

    pub fn matches_folder_name(&self, name: &str) -> bool {
        let lower = name.to_lowercase();
        let clean = lower.split(" (steam)").next().unwrap_or(&lower).trim();
        match self {
            ManagedProton::GeProton => {
                if clean.contains("rtsp") || clean.contains("dwproton") || clean.contains("cachyos")
                {
                    return false;
                }
                clean.starts_with("ge-proton")
                    || clean.starts_with("proton-ge")
                    || clean.starts_with("ge proton")
                    || clean.starts_with("proton ge")
                    || clean.contains("ge-proton")
                    || clean.contains("proton-ge")
                    || clean.contains("ge proton")
                    || clean.contains("proton ge")
            }
            ManagedProton::DwProton => {
                clean.starts_with("dwproton")
                    || clean.starts_with("dw-proton")
                    || clean.starts_with("dw proton")
                    || clean.starts_with("proton-dw")
                    || clean.contains("dwproton")
                    || clean.contains("dw-proton")
                    || clean.contains("dw proton")
            }
            ManagedProton::ProtonCachyos => {
                clean.starts_with("proton-cachyos")
                    || clean.starts_with("cachyos-proton")
                    || clean.starts_with("cachyos proton")
                    || clean.contains("cachyos")
            }
            ManagedProton::ProtonRtsp => {
                clean.starts_with("proton-rtsp") || clean.contains("rtsp")
            }
        }
    }

    /// Check if this Proton is currently installed in Doors' compatibilitytools.d.
    /// Returns (version_folder_name, full_directory_path).
    pub fn installed_version(&self) -> Option<(String, PathBuf)> {
        let doors_dir = get_doors_compatibilitytools_dir();
        let mut dirs = vec![doors_dir];
        let local_doors = real_user_home().join(".local/share/doors/compatibilitytools.d");
        if !dirs.contains(&local_doors) {
            dirs.push(local_doors);
        }
        for dir in dirs {
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                        if self.matches_folder_name(name)
                            && crate::proton::is_proton_dir_compatible_with_host(&p)
                        {
                            return Some((name.to_string(), p));
                        }
                    }
                }
            }
        }

        None
    }

    /// Fetch latest release metadata from GitHub API (or Forgejo fallback).
    pub fn fetch_latest_release(&self) -> anyhow::Result<ProtonReleaseInfo> {
        match self {
            ManagedProton::GeProton => {
                fetch_github_latest("GloriousEggroll/proton-ge-custom", |name| {
                    name.starts_with("GE-Proton") && is_compatible_host_archive(name)
                })
            }
            ManagedProton::DwProton => {
                // Try GitHub mirror first
                fetch_github_latest("dawn-winery/dwproton-mirror", |name| {
                    (name.starts_with("dwproton") || name.starts_with("DW-Proton"))
                        && is_compatible_host_archive(name)
                })
                .or_else(|_| {
                    // Fallback to Dawn Winery Forgejo instance
                    fetch_forgejo_latest("https://dawn.wine/api/v1/repos/dawn-winery/dwproton/releases/latest")
                })
            }
            ManagedProton::ProtonCachyos => {
                fetch_github_latest("CachyOS/proton-cachyos", |name| {
                    name.starts_with("proton-cachyos") && is_compatible_host_archive(name)
                })
            }
            ManagedProton::ProtonRtsp => {
                fetch_github_latest("SpookySkeletons/proton-rtsp", |name| {
                    (name.starts_with("proton-rtsp") || name.starts_with("Proton-RTSP"))
                        && is_compatible_host_archive(name)
                })
            }
        }
    }

    /// Download and extract the latest version into compatibilitytools.d.
    /// Removes any older versions of this runner and updates game configurations.
    pub fn install_latest<F>(&self, on_progress: F) -> anyhow::Result<String>
    where
        F: Fn(f64, &str) + Send + 'static,
    {
        on_progress(0.02, &tr("Checking latest release..."));
        let rel = self.fetch_latest_release()?;

        let dest_dir = get_doors_compatibilitytools_dir();
        fs::create_dir_all(&dest_dir)?;

        let tmp_dir = std::env::temp_dir().join("doors_proton_dl");
        let _ = fs::create_dir_all(&tmp_dir);
        let tmp_file = tmp_dir.join(format!("{}.part", rel.asset_name));
        let _ = fs::remove_file(&tmp_file);

        on_progress(0.05, &tr("Downloading..."));

        // Download using curl
        let mut curl_cmd = build_command("curl");
        curl_cmd
            .arg("-fSL")
            .arg("-H")
            .arg("User-Agent: Doors-App")
            .arg("-o")
            .arg(&tmp_file)
            .arg(&rel.asset_url);

        let mut child = curl_cmd.spawn()?;

        let expected_size = rel.size.max(1);
        while child.try_wait()?.is_none() {
            if let Ok(meta) = fs::metadata(&tmp_file) {
                let frac = (meta.len() as f64 / expected_size as f64).clamp(0.05, 0.90);
                let mb_curr = meta.len() as f64 / 1_048_576.0;
                let mb_total = expected_size as f64 / 1_048_576.0;
                let msg = format!("{} ({:.0} MB / {:.0} MB)", tr("Downloading..."), mb_curr, mb_total);
                on_progress(frac, &msg);
            }
            sleep(Duration::from_millis(300));
        }

        let status = child.wait()?;
        if !status.success() {
            let _ = fs::remove_file(&tmp_file);
            anyhow::bail!("Failed to download Proton release asset: exit code {:?}", status.code());
        }

        on_progress(0.92, &tr("Extracting..."));

        // Extract using tar
        let mut tar_cmd = build_command("tar");
        tar_cmd
            .arg("-xf")
            .arg(&tmp_file)
            .arg("-C")
            .arg(&dest_dir);

        let tar_status = tar_cmd.status()?;
        let _ = fs::remove_file(&tmp_file);

        if !tar_status.success() {
            anyhow::bail!("Failed to extract Proton archive: exit code {:?}", tar_status.code());
        }

        // Find the newly extracted folder name
        let expected_stem = rel
            .asset_name
            .trim_end_matches(".tar.xz")
            .trim_end_matches(".tar.gz")
            .trim_end_matches(".tar.zst");
        let mut installed_name = if dest_dir.join(expected_stem).is_dir() {
            expected_stem.to_string()
        } else {
            rel.tag_name.clone()
        };
        if !dest_dir.join(&installed_name).is_dir() {
            if let Ok(entries) = fs::read_dir(&dest_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                        if self.matches_folder_name(name)
                            && name.contains(&rel.tag_name)
                            && crate::proton::is_proton_dir_compatible_with_host(&p)
                        {
                            installed_name = name.to_string();
                            break;
                        }
                    }
                }
            }
        }

        // Remove any older or incompatible-arch versions of this runner from compatibilitytools.d
        if let Ok(entries) = fs::read_dir(&dest_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    if self.matches_folder_name(name) && name != installed_name {
                        eprintln!("Doors: Removing older Proton version: {:?}", p);
                        let _ = fs::remove_dir_all(&p);
                    }
                }
            }
        }

        // Update games that were configured with the previous version (only if using Doors' managed Proton)
        let disks = discover_disks();
        let games = discover_installed_games(&disks);
        for mut game in games {
            let is_doors_proton =
                game.proton_dir.starts_with(&dest_dir) || !game.proton_name.contains("(Steam)");
            if is_doors_proton && self.matches_folder_name(&game.proton_name) && game.proton_name != installed_name {
                game.proton_name = installed_name.clone();
                game.proton_dir = dest_dir.join(&installed_name);
                let _ = crate::desktop::save_config_and_create_desktop_entry(&game);
            }
        }

        on_progress(1.0, &tr("Completed!"));
        Ok(installed_name)
    }

    /// Delete all versions of this runner from Doors' compatibilitytools.d.
    /// Steam's directories are never touched.
    pub fn delete_installed(&self) -> anyhow::Result<()> {
        let mut dirs = vec![
            get_doors_compatibilitytools_dir(),
            real_user_home().join(".local/share/doors/compatibilitytools.d"),
        ];
        dirs.dedup();
        for dest_dir in dirs {
            if let Ok(entries) = fs::read_dir(&dest_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                        if self.matches_folder_name(name) {
                            let _ = fs::remove_dir_all(&p);
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

/// Returns true if `asset_name` is a supported tarball matching the host CPU architecture
/// (excluding ARM64 archives on x86_64 and microarchitecture-locked `x86_64_v2/v3/v4` variants).
pub(crate) fn is_compatible_host_archive(asset_name: &str) -> bool {
    let lower = asset_name.to_ascii_lowercase();
    let is_tar =
        lower.ends_with(".tar.xz") || lower.ends_with(".tar.gz") || lower.ends_with(".tar.zst");
    if !is_tar
        || lower.ends_with(".sha512sum")
        || lower.ends_with(".sha256sum")
        || lower.ends_with(".sum")
        || lower.ends_with(".sig")
    {
        return false;
    }

    if cfg!(target_arch = "x86_64") {
        if lower.contains("arm64")
            || lower.contains("aarch64")
            || lower.contains("x86_64_v")
            || lower.contains("x86-64-v")
        {
            return false;
        }
    } else if cfg!(target_arch = "aarch64") {
        if lower.contains("x86_64") || lower.contains("amd64") {
            return false;
        }
    }

    true
}

/// Doors' own compatibility tools directory inside its Flatpak app data (or ~/.local/share/doors/compatibilitytools.d)
pub fn get_doors_compatibilitytools_dir() -> PathBuf {
    let home = real_user_home();
    let flatpak_dir = home.join(format!(
        ".var/app/{}/data/compatibilitytools.d",
        crate::desktop::APP_ID
    ));
    let in_flatpak = Path::new("/.flatpak-info").exists();

    if in_flatpak || flatpak_dir.parent().map(|p| p.is_dir()).unwrap_or(false) {
        flatpak_dir
    } else {
        home.join(".local/share/doors/compatibilitytools.d")
    }
}

/// Helper to build a command: uses tools available directly inside the Flatpak runtime (`curl`, `tar`)
/// when present, falling back to `flatpak-spawn --host` only for host-only binaries (`lspci`).
pub fn build_command(binary: &str) -> Command {
    let in_flatpak = Path::new("/.flatpak-info").exists();
    if in_flatpak
        && !Path::new("/usr/bin").join(binary).is_file()
        && !Path::new("/bin").join(binary).is_file()
    {
        let mut cmd = Command::new("flatpak-spawn");
        cmd.arg("--host").arg(binary);
        cmd
    } else {
        Command::new(binary)
    }
}

/// Always execute `binary` on the host OS (via `flatpak-spawn --host` when running inside Flatpak).
pub fn build_host_command(binary: &str) -> Command {
    if Path::new("/.flatpak-info").exists() {
        let mut cmd = Command::new("flatpak-spawn");
        cmd.arg("--host").arg(binary);
        cmd
    } else {
        Command::new(binary)
    }
}

fn fetch_github_latest<F>(repo: &str, asset_matcher: F) -> anyhow::Result<ProtonReleaseInfo>
where
    F: Fn(&str) -> bool,
{
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let mut cmd = build_command("curl");
    cmd.arg("-sSL")
        .arg("-H")
        .arg("User-Agent: Doors-App")
        .arg("-H")
        .arg("Accept: application/vnd.github.v3+json")
        .arg(&url);

    let output = cmd.output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to query GitHub API for {repo}");
    }

    let json_text = String::from_utf8_lossy(&output.stdout);
    let val: serde_json::Value = serde_json::from_str(&json_text)?;

    let tag_name = val["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing tag_name in release"))?
        .to_string();

    let assets = val["assets"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Missing assets in release"))?;

    for asset in assets {
        let name = asset["name"].as_str().unwrap_or("");
        if asset_matcher(name) {
            let asset_url = asset["browser_download_url"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing browser_download_url"))?
                .to_string();
            let size = asset["size"].as_u64().unwrap_or(0);
            return Ok(ProtonReleaseInfo {
                tag_name,
                asset_url,
                asset_name: name.to_string(),
                size,
            });
        }
    }

    anyhow::bail!("No matching release archive found in {repo}")
}

fn fetch_forgejo_latest(url: &str) -> anyhow::Result<ProtonReleaseInfo> {
    let mut cmd = build_command("curl");
    cmd.arg("-sSL")
        .arg("-H")
        .arg("User-Agent: Doors-App")
        .arg(url);

    let output = cmd.output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to query Forgejo API: {url}");
    }

    let json_text = String::from_utf8_lossy(&output.stdout);
    let val: serde_json::Value = serde_json::from_str(&json_text)?;

    let tag_name = val["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing tag_name"))?
        .to_string();

    let assets = val["assets"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Missing assets"))?;

    for asset in assets {
        let name = asset["name"].as_str().unwrap_or("");
        if is_compatible_host_archive(name) {
            let asset_url = asset["browser_download_url"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing browser_download_url"))?
                .to_string();
            let size = asset["size"].as_u64().unwrap_or(0);
            return Ok(ProtonReleaseInfo {
                tag_name,
                asset_url,
                asset_name: name.to_string(),
                size,
            });
        }
    }

    anyhow::bail!("No matching release archive found in Forgejo API")
}

pub fn check_network_connectivity() -> bool {
    let mut cmd = build_command("curl");
    cmd.arg("-s")
        .arg("--head")
        .arg("--request")
        .arg("GET")
        .arg("--max-time")
        .arg("5")
        .arg("https://api.github.com");

    cmd.output().map(|o| o.status.success()).unwrap_or(false)
}

/// Ensure background boot & periodic Proton/UMU updater is registered.
/// Uses a `systemd --user` service + timer (`doors-updater.service` / `doors-updater.timer`,
/// matching ProtonPlus on Flathub so no `~/.config/autostart` Flatpak permission is needed),
/// falling back to `~/.config/autostart` only on non-systemd native Linux distributions.
pub fn ensure_autostart_updater() {
    let in_flatpak = Path::new("/.flatpak-info").exists();
    let home = real_user_home();

    let exec_start = if in_flatpak {
        let flatpak_bin = build_host_command("sh")
            .arg("-c")
            .arg("command -v flatpak")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/usr/bin/flatpak".to_string());
        format!(
            "{flatpak_bin} run {} --update-protons-bg",
            crate::desktop::APP_ID
        )
    } else {
        let self_exe = std::env::current_exe()
            .ok()
            .map(|p| format!("\"{}\"", p.display()))
            .unwrap_or_else(|| "doors".to_string());
        format!("{self_exe} --update-protons-bg")
    };

    let systemd_user_dir = home.join(".config/systemd/user");
    let mut systemd_enabled = false;
    if fs::create_dir_all(&systemd_user_dir).is_ok() {
        let service_content = format!(
            "[Unit]\n\
             Description=Doors Background Proton and UMU Updater\n\
             After=network-online.target\n\
             \n\
             [Service]\n\
             Type=oneshot\n\
             ExecStart={exec_start}\n"
        );
        let timer_content = "\
[Unit]\n\
Description=Doors Background Proton and UMU Updater Timer\n\
\n\
[Timer]\n\
OnBootSec=1min\n\
OnUnitActiveSec=12h\n\
\n\
[Install]\n\
WantedBy=timers.target\n";

        let service_path = systemd_user_dir.join("doors-updater.service");
        let timer_path = systemd_user_dir.join("doors-updater.timer");

        if fs::write(&service_path, service_content).is_ok()
            && fs::write(&timer_path, timer_content).is_ok()
        {
            let reload_ok = build_host_command("systemctl")
                .args(["--user", "daemon-reload"])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            let enable_ok = reload_ok
                && build_host_command("systemctl")
                    .args(["--user", "enable", "doors-updater.timer"])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);
            if enable_ok {
                let _ = build_host_command("systemctl")
                    .args(["--user", "start", "doors-updater.timer"])
                    .output();
                systemd_enabled = true;
            }
        }
    }

    if systemd_enabled {
        // Clean up legacy autostart entries on native installs so updates don't run twice on boot
        if !in_flatpak {
            let autostart_dir = home.join(".config/autostart");
            let _ = fs::remove_file(autostart_dir.join("io.github.doors.Doors.updater.desktop"));
            let _ = fs::remove_file(
                autostart_dir.join(format!("{}.updater.desktop", crate::desktop::APP_ID)),
            );
        }
        return;
    }

    // Fallback for non-systemd native Linux distributions (OpenRC, runit, dinit, etc.)
    if !in_flatpak {
        let autostart_dir = home.join(".config/autostart");
        let _ = fs::create_dir_all(&autostart_dir);
        let _ = fs::remove_file(autostart_dir.join("io.github.doors.Doors.updater.desktop"));
        let autostart_file =
            autostart_dir.join(format!("{}.updater.desktop", crate::desktop::APP_ID));

        let content = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Doors Proton Updater\n\
             Comment=Check and update installed Protons in background\n\
             Exec={exec_start}\n\
             Terminal=false\n\
             Hidden=false\n\
             NoDisplay=true\n\
             X-GNOME-Autostart-enabled=true\n"
        );

        let _ = fs::write(autostart_file, content);
    }
}

/// Runs silently in the background on PC startup or via CLI.
/// Checks installed runners and updates them if a newer version is available.
pub fn run_background_update() -> anyhow::Result<()> {
    if BG_UPDATE_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(());
    }
    let compat_dir = get_doors_compatibilitytools_dir();
    let _ = fs::create_dir_all(&compat_dir);
    let lock_path = compat_dir.join(".doors-bg-update.lock");
    if let Ok(meta) = fs::metadata(&lock_path) {
        if let Ok(modified) = meta.modified() {
            if modified.elapsed().unwrap_or(Duration::from_secs(3600)) < Duration::from_secs(1800) {
                BG_UPDATE_RUNNING.store(false, Ordering::SeqCst);
                return Ok(());
            }
        }
        let _ = fs::remove_file(&lock_path);
    }
    if fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .is_err()
    {
        BG_UPDATE_RUNNING.store(false, Ordering::SeqCst);
        return Ok(());
    }

    struct UpdateGuard(PathBuf);
    impl Drop for UpdateGuard {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
            BG_UPDATE_RUNNING.store(false, Ordering::SeqCst);
        }
    }
    let _guard = UpdateGuard(lock_path);

    // Retry checking network for up to 30 seconds if PC just booted
    let mut online = false;
    for _ in 0..6 {
        if check_network_connectivity() {
            online = true;
            break;
        }
        sleep(Duration::from_secs(5));
    }

    if !online {
        eprintln!("Doors Proton Updater: No internet connection after boot, skipping.");
        return Ok(());
    }

    for runner in &ManagedProton::ALL {
        if let Some((current_ver, _)) = runner.installed_version() {
            eprintln!(
                "Doors Proton Updater: Checking update for {} (current: {})...",
                runner.display_name(),
                current_ver
            );
            if let Ok(latest) = runner.fetch_latest_release() {
                if !current_ver.contains(&latest.tag_name) && latest.tag_name != current_ver {
                    eprintln!(
                        "Doors Proton Updater: Found new version {} for {}, downloading...",
                        latest.tag_name,
                        runner.display_name()
                    );
                    if let Err(e) = runner.install_latest(|_, _| {}) {
                        eprintln!(
                            "Doors Proton Updater: Failed to update {}: {}",
                            runner.display_name(),
                            e
                        );
                    } else {
                        eprintln!(
                            "Doors Proton Updater: Successfully updated {} to {}",
                            runner.display_name(),
                            latest.tag_name
                        );
                    }
                } else {
                    eprintln!(
                        "Doors Proton Updater: {} is up to date ({})",
                        runner.display_name(),
                        current_ver
                    );
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_managed_proton_matches() {
        assert!(ManagedProton::GeProton.matches_folder_name("GE-Proton9-25"));
        assert!(ManagedProton::GeProton.matches_folder_name("ge-proton10-1"));
        assert!(ManagedProton::GeProton.matches_folder_name("Proton-GE Latest"));
        assert!(ManagedProton::GeProton.matches_folder_name("Proton-GE Latest (Steam)"));
        assert!(ManagedProton::GeProton.matches_folder_name("proton-ge-custom"));
        assert!(!ManagedProton::GeProton.matches_folder_name("Proton 9.0"));
        assert!(!ManagedProton::GeProton.matches_folder_name("Proton - Experimental"));

        assert!(ManagedProton::DwProton.matches_folder_name("dwproton-11.0-14-x86_64"));
        assert!(ManagedProton::DwProton.matches_folder_name("DW-Proton-10"));
        assert!(ManagedProton::DwProton.matches_folder_name("DW-Proton Latest (Steam)"));
        assert!(!ManagedProton::DwProton.matches_folder_name("GE-Proton9-25"));

        assert!(ManagedProton::ProtonCachyos.matches_folder_name("proton-cachyos-11.0-20250101"));
        assert!(ManagedProton::ProtonCachyos.matches_folder_name("Proton-CachyOS (Steam)"));
        assert!(ManagedProton::ProtonCachyos.matches_folder_name("cachyos-proton"));
        assert!(!ManagedProton::ProtonCachyos.matches_folder_name("Proton-9.0"));

        assert!(ManagedProton::ProtonRtsp.matches_folder_name("proton-rtsp-GE-Proton9-20"));
        assert!(ManagedProton::ProtonRtsp.matches_folder_name("GE-Proton9-22-rtsp17-1"));
        assert!(ManagedProton::ProtonRtsp.matches_folder_name("Proton-RTSP (Steam)"));
        assert!(!ManagedProton::ProtonRtsp.matches_folder_name("GE-Proton9-25"));
        assert!(!ManagedProton::GeProton.matches_folder_name("proton-rtsp-GE-Proton9-20"));
        assert!(!ManagedProton::GeProton.matches_folder_name("GE-Proton9-22-rtsp17-1"));
    }

    #[test]
    fn test_compatibilitytools_dir() {
        let dir = get_doors_compatibilitytools_dir();
        assert!(dir.ends_with("compatibilitytools.d"));
    }
}

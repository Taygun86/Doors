use crate::i18n::tr;
use std::collections::HashSet;
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct DiskInfo {
    /// Display name, e.g. "Main Disk" or "HDD"
    pub label: String,
    /// Path where Doors stores games on this disk, e.g. "/mnt/HDD/Doors"
    pub doors_root: PathBuf,
    /// Free bytes available to non-root user
    pub free_bytes: u64,
}

impl DiskInfo {
    pub fn display_row(&self) -> String {
        let free_gb = self.free_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
        format!("{} ({:.1} GB {})", self.label, free_gb, tr("free"))
    }
}

pub fn statvfs_space(path: &Path) -> Option<(u64, u64)> {
    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c_path.as_ptr(), &mut stat) == 0 {
            let frsize = if stat.f_frsize > 0 {
                stat.f_frsize as u64
            } else {
                stat.f_bsize as u64
            };
            let free = (stat.f_bavail as u64).saturating_mul(frsize);
            let total = (stat.f_blocks as u64).saturating_mul(frsize);
            Some((free, total))
        } else {
            None
        }
    }
}

/// Discover all usable disks:
/// 1. User's Home disk (`~/Doors`)
/// 2. Mounted drives under `/mnt/*`, `/run/media/*/*`, `/media/*/*`
/// 3. Physical filesystems from `/proc/mounts` mounted under `/mnt`, `/run/media`, `/media`
pub fn discover_disks() -> Vec<DiskInfo> {
    let mut disks = Vec::new();
    let mut seen_devs = HashSet::new();
    let mut seen_paths = HashSet::new();

    let root_dev = fs::metadata("/").map(|m| m.dev()).ok();

    // 1. Home directory (Ana Disk)
    let home = real_user_home();
    if let Ok(meta) = fs::metadata(&home) {
        if let Some((free, total)) = statvfs_space(&home) {
            if total > 0 {
                seen_devs.insert(meta.dev());
                seen_paths.insert(home.clone());
                disks.push(DiskInfo {
                    label: tr("Main Disk"),
                    doors_root: home.join("Doors"),
                    free_bytes: free,
                });
            }
        }
    }

    // Collect candidate mount directories from /mnt, /run/media, /media, and /proc/mounts
    let mut candidates: Vec<PathBuf> = Vec::new();

    // Scan /mnt subdirectories
    if let Ok(entries) = fs::read_dir("/mnt") {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                candidates.push(path);
            }
        }
    }
    // Also check if /mnt itself is a separate mounted disk
    if let Ok(meta) = fs::metadata("/mnt") {
        if Some(meta.dev()) != root_dev && !seen_devs.contains(&meta.dev()) {
            candidates.push(PathBuf::from("/mnt"));
        }
    }

    // Scan /run/media/<user>/* and /media/<user>/*
    for base in ["/run/media", "/media"] {
        if let Ok(users) = fs::read_dir(base) {
            for user_entry in users.flatten() {
                let user_path = user_entry.path();
                if user_path.is_dir() {
                    if let Ok(mounts) = fs::read_dir(&user_path) {
                        for mount_entry in mounts.flatten() {
                            let mp = mount_entry.path();
                            if mp.is_dir() {
                                candidates.push(mp);
                            }
                        }
                    }
                }
            }
        }
    }

    // Parse /proc/mounts for any extra mounts under /mnt, /run/media, /media
    if let Ok(mounts_str) = fs::read_to_string("/proc/mounts") {
        for line in mounts_str.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                let mp_raw = parts[1].replace("\\040", " ");
                let fstype = parts[2];
                if (mp_raw.starts_with("/mnt")
                    || mp_raw.starts_with("/run/media")
                    || mp_raw.starts_with("/media")
                    || mp_raw.starts_with("/var/mnt"))
                    && !matches!(
                        fstype,
                        "tmpfs" | "devtmpfs" | "proc" | "sysfs" | "cgroup2" | "overlay" | "squashfs"
                    )
                {
                    candidates.push(PathBuf::from(mp_raw));
                }
            }
        }
    }

    for path in candidates {
        let canonical = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if seen_paths.contains(&canonical) {
            continue;
        }
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        let Some((free, total)) = statvfs_space(&path) else {
            continue;
        };
        if total == 0 {
            continue;
        }

        // Include if it's a different block device OR an explicit folder inside /mnt
        let is_mnt_subdir = path.parent() == Some(Path::new("/mnt"));
        if seen_devs.contains(&meta.dev()) && !is_mnt_subdir {
            continue;
        }

        seen_devs.insert(meta.dev());
        seen_paths.insert(canonical);

        let folder_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Disk");

        disks.push(DiskInfo {
            label: folder_name.to_string(),
            doors_root: path.join("Doors"),
            free_bytes: free,
        });
    }

    disks
}

/// Returns the real host user home directory even when running inside Flatpak
/// (where `HOME` might be overridden or `$XDG_DATA_HOME` points to `~/.var/app/...`).
pub fn real_user_home() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(&home);
        if p.exists() {
            return p;
        }
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"))
}

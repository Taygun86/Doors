use std::fs::{self, File};
use std::io::{Cursor, Read};
use std::path::Path;
use walkdir::WalkDir;

#[derive(Debug, Clone)]
struct CandidateIcon {
    width: u32,
    height: u32,
    bpp: u16,
    png_bytes: Vec<u8>,
}

/// Embedded metadata extracted directly from a Windows PE (`.exe`) resource table (`RT_VERSION` & `RT_MANIFEST`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExeMetadata {
    pub product_name: Option<String>,
    pub file_description: Option<String>,
    pub company_name: Option<String>,
    pub internal_name: Option<String>,
    pub original_filename: Option<String>,
    pub manifest_description: Option<String>,
    pub manifest_is_installer: bool,
}

impl ExeMetadata {
    /// Returns true if the embedded PE metadata or manifest explicitly identifies this `.exe` as a setup/installer binary.
    pub fn is_installer_binary(&self) -> bool {
        if self.manifest_is_installer {
            return true;
        }
        for field in [
            self.file_description.as_deref(),
            self.internal_name.as_deref(),
            self.original_filename.as_deref(),
            self.manifest_description.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            let lower = field
                .trim()
                .trim_end_matches(".exe")
                .trim_end_matches(".EXE")
                .trim()
                .to_lowercase();
            if lower == "setup"
                || lower == "install"
                || lower == "installer"
                || lower.ends_with(" setup")
                || lower.ends_with(" installer")
                || lower.contains("inno setup")
                || lower.contains("nullsoft install")
                || lower.contains("installshield")
                || lower.contains("self-extracting")
            {
                return true;
            }
        }
        false
    }
    /// Returns the best non-generic application/game title embedded in the binary.
    pub fn best_title(&self) -> Option<String> {
        for cand in [
            self.product_name.as_deref(),
            self.file_description.as_deref(),
            self.manifest_description.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(cleaned) = sanitize_metadata_title(cand) {
                return Some(cleaned);
            }
        }
        None
    }

    /// Returns all meaningful embedded string signals for database matching.
    pub fn candidate_signals(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut push_unique = |s: &str| {
            let trimmed = s
                .trim()
                .trim_end_matches(".exe")
                .trim_end_matches(".EXE")
                .trim();
            if trimmed.len() >= 2
                && !is_generic_metadata_boilerplate(trimmed)
                && !out.iter().any(|existing: &String| existing.eq_ignore_ascii_case(trimmed))
            {
                out.push(trimmed.to_string());
            }
        };

        if let Some(ref s) = self.product_name {
            push_unique(s);
        }
        if let Some(ref s) = self.file_description {
            push_unique(s);
        }
        if let Some(ref s) = self.internal_name {
            push_unique(s);
        }
        if let Some(ref s) = self.original_filename {
            push_unique(s);
        }
        if let Some(ref s) = self.manifest_description {
            push_unique(s);
        }
        out
    }
}

pub fn is_generic_metadata_boilerplate(raw: &str) -> bool {
    let lower = raw
        .trim()
        .trim_end_matches(".exe")
        .trim_end_matches(".EXE")
        .trim()
        .to_lowercase();

    if lower.len() < 2 {
        return true;
    }

    let exact_generic = [
        "setup",
        "install",
        "installer",
        "uninstall",
        "uninstaller",
        "autorun",
        "launcher",
        "client",
        "game",
        "app",
        "application",
        "program",
        "update",
        "updater",
        "patch",
        "patcher",
        "bootstrapper",
        "downloader",
        "unity",
        "unity player",
        "unityplayer",
        "unreal engine",
        "unrealgame",
        "ue4game",
        "ue5game",
        "bootstrappackagedgame",
        "godot engine",
        "gamemaker",
        "renpy",
        "nwjs",
        "electron",
        "7-zip",
        "7z",
        "sfx",
    ];
    if exact_generic.contains(&lower.as_str()) {
        return true;
    }

    let boilerplate_substrings = [
        "inno setup",
        "nullsoft install",
        "installshield",
        "windows installer",
        "wix toolset",
        "self-extracting",
        "setup/uninstall",
        "microsoft(r) windows",
        "microsoft windows operating system",
        "todo: <",
        "package stub",
        "archive extractor",
    ];
    boilerplate_substrings.iter().any(|pat| lower.contains(pat))
}

fn sanitize_metadata_title(raw: &str) -> Option<String> {
    let trimmed = raw
        .trim_matches(|c: char| c.is_control() || c.is_whitespace())
        .replace('™', "")
        .replace('®', "")
        .replace('©', "");
    if is_generic_metadata_boilerplate(&trimmed) {
        return None;
    }

    let cleaned = crate::detector::clean_game_title(&trimmed);
    if cleaned.len() < 2 || is_generic_metadata_boilerplate(&cleaned) {
        return None;
    }
    Some(cleaned)
}

impl CandidateIcon {
    fn score(&self) -> u64 {
        let pixels = (self.width as u64) * (self.height as u64);
        pixels * 100 + (self.bpp as u64)
    }
}

/// Extract the best icon from `exe_path` (`.exe`, `.msi`, `.ico`, or `.png`, or sibling files in `game_dir`)
/// and return the raw `.png` bytes in memory.
pub fn extract_exe_icon_png_bytes(exe_path: &Path, game_dir: Option<&Path>) -> Option<Vec<u8>> {
    // 1. Try extracting directly from `.ico`, `.png`, `.exe`, or `.msi`
    if let Some(ext) = exe_path.extension().and_then(|e| e.to_str()) {
        if ext.eq_ignore_ascii_case("ico") {
            if let Some(best) = extract_best_icon_from_ico_file(exe_path) {
                return Some(best.png_bytes);
            }
        } else if ext.eq_ignore_ascii_case("png") {
            if let Ok(raw) = fs::read(exe_path) {
                if raw.len() > 24 && &raw[0..8] == b"\x89PNG\r\n\x1a\n" {
                    return Some(raw);
                }
            }
        }
    }

    if let Some(best) = extract_best_icon_from_pe_file(exe_path)
        .or_else(|| extract_best_icon_from_msi_file(exe_path))
    {
        return Some(best.png_bytes);
    }

    // 2. Fallback: look for .ico files or other .exe files ONLY inside an explicit installed `game_dir`
    if let Some(dir) = game_dir {
        if crate::detector::is_protected_directory(dir) {
            return None;
        }
        let mut best_fallback: Option<CandidateIcon> = None;

        for entry in WalkDir::new(dir).max_depth(3).into_iter().flatten() {
            let path = entry.path();
            if !path.is_file() || path == exe_path {
                continue;
            }
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if ext.eq_ignore_ascii_case("ico") {
                if let Some(cand) = extract_best_icon_from_ico_file(path) {
                    if best_fallback
                        .as_ref()
                        .map_or(true, |b| cand.score() > b.score())
                    {
                        best_fallback = Some(cand);
                    }
                }
            } else if ext.eq_ignore_ascii_case("exe") {
                if crate::detector::is_auxiliary_exe(path) {
                    continue;
                }
                if let Some(cand) = extract_best_icon_from_pe_file(path) {
                    if best_fallback
                        .as_ref()
                        .map_or(true, |b| cand.score() > b.score())
                    {
                        best_fallback = Some(cand);
                    }
                }
            }
        }

        if let Some(best) = best_fallback {
            return Some(best.png_bytes);
        }
    }

    None
}

/// Extract the best icon from `exe_path` (or sibling `.ico`/`.exe` files in `game_dir`)
/// and save it as a `.png` at `out_png_path`.
pub fn extract_exe_icon_to_png(
    exe_path: &Path,
    game_dir: Option<&Path>,
    out_png_path: &Path,
) -> bool {
    if let Some(parent) = out_png_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if let Some(png_bytes) = extract_exe_icon_png_bytes(exe_path, game_dir) {
        if fs::write(out_png_path, &png_bytes).is_ok() {
            return true;
        }
    }

    false
}

/// Check if a PE `.exe` file contains at least one embedded icon resource.
pub fn pe_has_icon(exe_path: &Path) -> bool {
    extract_best_icon_from_pe_file(exe_path).is_some()
}

/// Return the `(width, height)` of the best icon inside a `.ico`, `.png`, `.exe`, or `.msi` file.
pub fn icon_source_dimensions(path: &Path) -> Option<(u32, u32)> {
    if !path.is_file() {
        return None;
    }
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if ext.eq_ignore_ascii_case("ico") {
            return extract_best_icon_from_ico_file(path).map(|c| (c.width, c.height));
        }
        if ext.eq_ignore_ascii_case("png") {
            let raw = fs::read(path).ok()?;
            if raw.len() > 24 && &raw[0..8] == b"\x89PNG\r\n\x1a\n" {
                let w = u32::from_be_bytes([raw[16], raw[17], raw[18], raw[19]]);
                let h = u32::from_be_bytes([raw[20], raw[21], raw[22], raw[23]]);
                if w > 0 && h > 0 {
                    return Some((w, h));
                }
            }
            return None;
        }
    }
    extract_best_icon_from_pe_file(path)
        .or_else(|| extract_best_icon_from_msi_file(path))
        .map(|c| (c.width, c.height))
}

fn extract_best_icon_from_ico_file(ico_path: &Path) -> Option<CandidateIcon> {
    let file = File::open(ico_path).ok()?;
    let icon_dir = ico::IconDir::read(file).ok()?;
    let mut best: Option<CandidateIcon> = None;

    for entry in icon_dir.entries() {
        if let Ok(image) = entry.decode() {
            let mut png_buf = Vec::new();
            if image.write_png(&mut png_buf).is_ok() {
                let cand = CandidateIcon {
                    width: image.width(),
                    height: image.height(),
                    bpp: entry.bits_per_pixel(),
                    png_bytes: png_buf,
                };
                if best.as_ref().map_or(true, |b| cand.score() > b.score()) {
                    best = Some(cand);
                }
            }
        }
    }
    best
}

/// Extract an embedded `.ico` stream from a Windows Installer (`.msi` OLE2 Compound Document) if one is present.
fn extract_best_icon_from_msi_file(msi_path: &Path) -> Option<CandidateIcon> {
    let ext = msi_path.extension()?.to_str()?;
    if !ext.eq_ignore_ascii_case("msi") {
        return None;
    }

    let mut file = File::open(msi_path).ok()?;
    let file_len = file.metadata().ok()?.len() as usize;
    if file_len < 512 {
        return None;
    }

    let read_len = file_len.min(32 * 1024 * 1024);
    let mut buf = vec![0u8; read_len];
    file.read_exact(&mut buf).ok()?;

    // OLE2 magic header: D0 CF 11 E0 A1 B1 1A E1
    if &buf[0..8] != b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1" {
        return None;
    }

    let mut best: Option<CandidateIcon> = None;

    // OLE2 streams and mini-streams are aligned to 64-byte boundaries
    let mut off = 512;
    while off + 22 < buf.len() {
        // Check for ICO header: Reserved=0, Type=1, Count=1..=32
        if buf[off] == 0
            && buf[off + 1] == 0
            && buf[off + 2] == 1
            && buf[off + 3] == 0
            && (1..=32).contains(&buf[off + 4])
            && buf[off + 5] == 0
        {
            let slice_end = (off + 512 * 1024).min(buf.len());
            if let Ok(icon_dir) = ico::IconDir::read(Cursor::new(&buf[off..slice_end])) {
                for entry in icon_dir.entries() {
                    if let Ok(image) = entry.decode() {
                        let mut png_buf = Vec::new();
                        if image.write_png(&mut png_buf).is_ok() {
                            let cand = CandidateIcon {
                                width: image.width(),
                                height: image.height(),
                                bpp: entry.bits_per_pixel(),
                                png_bytes: png_buf,
                            };
                            if best.as_ref().map_or(true, |b| cand.score() > b.score()) {
                                best = Some(cand);
                            }
                        }
                    }
                }
            }
        }
        off += 64;
    }

    best
}

struct PeRsrcSection {
    buf: Vec<u8>,
    virtual_address: u32,
    root_off: usize,
}

impl PeRsrcSection {
    fn rva_to_buf_offset(&self, rva: u32) -> Option<usize> {
        if rva >= self.virtual_address {
            let off = (rva - self.virtual_address) as usize;
            if off < self.buf.len() {
                return Some(off);
            }
        }
        None
    }

    fn collect_resources_of_type(&self, target_type_id: u32) -> Vec<&[u8]> {
        let mut results = Vec::new();
        let Some(l1_entries) = parse_resource_dir(&self.buf, self.root_off) else {
            return results;
        };

        for (type_id, is_dir_l1, offset_l1) in l1_entries {
            if type_id != Some(target_type_id) || !is_dir_l1 {
                continue;
            }
            let Some(l2_off) = self.root_off.checked_add(offset_l1) else {
                continue;
            };
            let Some(l2_entries) = parse_resource_dir(&self.buf, l2_off) else {
                continue;
            };

            for (_res_id, is_dir_l2, offset_l2) in l2_entries {
                if !is_dir_l2 {
                    continue;
                }
                let Some(l3_off) = self.root_off.checked_add(offset_l2) else {
                    continue;
                };
                let Some(l3_entries) = parse_resource_dir(&self.buf, l3_off) else {
                    continue;
                };

                for (_lang_id, is_dir_l3, offset_l3) in l3_entries {
                    if is_dir_l3 {
                        continue;
                    }
                    let Some(data_entry_off) = self.root_off.checked_add(offset_l3) else {
                        continue;
                    };
                    if data_entry_off + 16 > self.buf.len() {
                        continue;
                    }
                    let Some(data_rva) = read_u32_le(&self.buf, data_entry_off) else {
                        continue;
                    };
                    let Some(data_size_u32) = read_u32_le(&self.buf, data_entry_off + 4) else {
                        continue;
                    };
                    let data_size = data_size_u32 as usize;

                    let Some(res_offset) = self.rva_to_buf_offset(data_rva) else {
                        continue;
                    };
                    let Some(end_offset) = res_offset.checked_add(data_size) else {
                        continue;
                    };
                    if end_offset <= self.buf.len() && data_size > 0 {
                        results.push(&self.buf[res_offset..end_offset]);
                    }
                }
            }
        }

        results
    }
}

fn load_pe_rsrc_section(exe_path: &Path) -> Option<PeRsrcSection> {
    let mut file = File::open(exe_path).ok()?;
    let file_len = file.metadata().ok()?.len();
    if file_len < 0x100 {
        return None;
    }

    // Read first 4 KB for DOS + PE + Section headers
    let mut header_buf = vec![0u8; 4096.min(file_len as usize)];
    file.read_exact(&mut header_buf).ok()?;

    if &header_buf[0..2] != b"MZ" {
        return None;
    }

    let pe_offset = read_u32_le(&header_buf, 0x3C)? as usize;
    if pe_offset + 24 > header_buf.len() {
        return None;
    }
    if &header_buf[pe_offset..pe_offset + 4] != b"PE\0\0" {
        return None;
    }

    let num_sections = read_u16_le(&header_buf, pe_offset + 6)? as usize;
    let opt_header_size = read_u16_le(&header_buf, pe_offset + 20)? as usize;
    let opt_offset = pe_offset + 24;

    if opt_offset + opt_header_size > header_buf.len() {
        return None;
    }

    let magic = read_u16_le(&header_buf, opt_offset)?;
    let data_dir_offset = match magic {
        0x10b => opt_offset + 96,  // PE32
        0x20b => opt_offset + 112, // PE32+ (64-bit)
        _ => return None,
    };

    // DataDirectory[2] is Resource Table (each entry is 8 bytes: RVA + Size)
    let rsrc_dir_entry_offset = data_dir_offset + 2 * 8;
    if rsrc_dir_entry_offset + 8 > opt_offset + opt_header_size {
        return None;
    }

    let rsrc_rva = read_u32_le(&header_buf, rsrc_dir_entry_offset)?;
    let rsrc_size = read_u32_le(&header_buf, rsrc_dir_entry_offset + 4)?;
    if rsrc_rva == 0 || rsrc_size == 0 {
        return None;
    }

    let sections_offset = opt_offset + opt_header_size;
    if sections_offset + num_sections * 40 > header_buf.len() {
        return None;
    }

    #[derive(Clone, Copy)]
    struct PeSection {
        virtual_address: u32,
        virtual_size: u32,
        raw_data_ptr: u32,
        raw_data_size: u32,
    }

    let mut sections = Vec::with_capacity(num_sections);
    for i in 0..num_sections {
        let s_off = sections_offset + i * 40;
        let virtual_size = read_u32_le(&header_buf, s_off + 8)?;
        let virtual_address = read_u32_le(&header_buf, s_off + 12)?;
        let raw_data_size = read_u32_le(&header_buf, s_off + 16)?;
        let raw_data_ptr = read_u32_le(&header_buf, s_off + 20)?;
        sections.push(PeSection {
            virtual_address,
            virtual_size: virtual_size.max(raw_data_size),
            raw_data_ptr,
            raw_data_size,
        });
    }

    let rsrc_sec = sections.iter().find(|s| {
        rsrc_rva >= s.virtual_address && rsrc_rva < s.virtual_address.saturating_add(s.virtual_size)
    })?;

    let sec_file_offset = rsrc_sec.raw_data_ptr as u64;
    let sec_read_size = (rsrc_sec.raw_data_size as usize).min(32 * 1024 * 1024);
    if sec_file_offset + (sec_read_size as u64) > file_len {
        return None;
    }

    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(sec_file_offset)).ok()?;
    let mut rsrc_buf = vec![0u8; sec_read_size];
    file.read_exact(&mut rsrc_buf).ok()?;

    let sec = PeRsrcSection {
        buf: rsrc_buf,
        virtual_address: rsrc_sec.virtual_address,
        root_off: 0,
    };
    let root_off = sec.rva_to_buf_offset(rsrc_rva)?;
    Some(PeRsrcSection { root_off, ..sec })
}

fn extract_best_icon_from_pe_file(exe_path: &Path) -> Option<CandidateIcon> {
    let rsrc = load_pe_rsrc_section(exe_path)?;
    let mut best_icon: Option<CandidateIcon> = None;

    // Resource Type 3 == RT_ICON
    for raw_icon in rsrc.collect_resources_of_type(3) {
        if raw_icon.len() < 16 {
            continue;
        }
        if let Some(cand) = decode_rt_icon_bytes(raw_icon) {
            if best_icon
                .as_ref()
                .map_or(true, |b| cand.score() > b.score())
            {
                best_icon = Some(cand);
            }
        }
    }

    best_icon
}

/// Extract embedded `VS_VERSION_INFO` (`RT_VERSION`, Type 16) and `RT_MANIFEST` (Type 24) metadata from a PE `.exe`.
pub fn extract_exe_metadata(exe_path: &Path) -> ExeMetadata {
    let mut meta = ExeMetadata::default();
    let Some(rsrc) = load_pe_rsrc_section(exe_path) else {
        return meta;
    };

    // Resource Type 16 == RT_VERSION
    for raw_ver in rsrc.collect_resources_of_type(16) {
        parse_vs_version_info(raw_ver, &mut meta);
        if meta.product_name.is_some() || meta.file_description.is_some() {
            break;
        }
    }

    // Resource Type 24 == RT_MANIFEST
    for raw_manifest in rsrc.collect_resources_of_type(24) {
        let manifest_lower = String::from_utf8_lossy(raw_manifest).to_ascii_lowercase();
        if manifest_lower.contains("inno.setup")
            || manifest_lower.contains("inno setup")
            || manifest_lower.contains("nullsoft.nsis")
            || manifest_lower.contains("nullsoft install")
            || manifest_lower.contains("installshield")
        {
            meta.manifest_is_installer = true;
        }
        if meta.manifest_description.is_none() {
            if let Some(desc) = parse_manifest_description(raw_manifest) {
                meta.manifest_description = Some(desc);
            }
        }
    }

    meta
}

fn parse_vs_version_info(raw: &[u8], meta: &mut ExeMetadata) {
    let _ = parse_version_block(raw, 0, 0, meta);

    // Fallback aligned UTF-16LE String struct lookup in case a non-MSVC resource compiler wrote quirky outer block lengths
    if meta.product_name.is_none() {
        meta.product_name = scan_utf16_version_string(raw, "ProductName");
    }
    if meta.file_description.is_none() {
        meta.file_description = scan_utf16_version_string(raw, "FileDescription");
    }
    if meta.company_name.is_none() {
        meta.company_name = scan_utf16_version_string(raw, "CompanyName");
    }
    if meta.internal_name.is_none() {
        meta.internal_name = scan_utf16_version_string(raw, "InternalName");
    }
    if meta.original_filename.is_none() {
        meta.original_filename = scan_utf16_version_string(raw, "OriginalFilename");
    }
}

fn parse_version_block(
    buf: &[u8],
    start: usize,
    depth: usize,
    meta: &mut ExeMetadata,
) -> Option<usize> {
    if depth > 5 {
        return None;
    }
    let off = (start + 3) & !3;
    if off + 6 > buf.len() {
        return None;
    }

    let w_len = read_u16_le(buf, off)? as usize;
    if w_len < 6 {
        return None;
    }
    let block_end = off.checked_add(w_len)?.min(buf.len());
    let w_val_len = read_u16_le(buf, off + 2)? as usize;
    let _w_type = read_u16_le(buf, off + 4)?;

    let mut pos = off + 6;
    let mut key_u16 = Vec::new();
    while pos + 2 <= block_end {
        let ch = read_u16_le(buf, pos)?;
        pos += 2;
        if ch == 0 {
            break;
        }
        key_u16.push(ch);
    }
    let key = String::from_utf16_lossy(&key_u16);
    pos = (pos + 3) & !3;

    if depth == 0 && key == "VS_VERSION_INFO" {
        pos = pos.saturating_add(w_val_len);
        pos = (pos + 3) & !3;
        while pos + 6 <= block_end {
            let Some(next) = parse_version_block(buf, pos, 1, meta) else {
                break;
            };
            if next <= pos {
                break;
            }
            pos = (next + 3) & !3;
        }
    } else if depth == 1 && key == "StringFileInfo" {
        while pos + 6 <= block_end {
            let Some(next) = parse_version_block(buf, pos, 2, meta) else {
                break;
            };
            if next <= pos {
                break;
            }
            pos = (next + 3) & !3;
        }
    } else if depth == 2 {
        // StringTable (e.g. "040904B0")
        while pos + 6 <= block_end {
            let Some(next) = parse_version_block(buf, pos, 3, meta) else {
                break;
            };
            if next <= pos {
                break;
            }
            pos = (next + 3) & !3;
        }
    } else if depth == 3 && w_val_len > 0 && pos + 2 <= block_end {
        let mut val_u16 = Vec::new();
        let mut vpos = pos;
        while vpos + 2 <= block_end {
            let ch = read_u16_le(buf, vpos)?;
            vpos += 2;
            if ch == 0 {
                break;
            }
            val_u16.push(ch);
        }
        let val = String::from_utf16_lossy(&val_u16)
            .trim_matches(|c: char| c.is_control() || c.is_whitespace())
            .to_string();
        if !val.is_empty() {
            assign_version_kv(meta, &key, val);
        }
    }

    Some(block_end)
}

fn assign_version_kv(meta: &mut ExeMetadata, key: &str, val: String) {
    match key {
        "ProductName" if meta.product_name.is_none() => meta.product_name = Some(val),
        "FileDescription" if meta.file_description.is_none() => meta.file_description = Some(val),
        "CompanyName" if meta.company_name.is_none() => meta.company_name = Some(val),
        "InternalName" if meta.internal_name.is_none() => meta.internal_name = Some(val),
        "OriginalFilename" if meta.original_filename.is_none() => {
            meta.original_filename = Some(val)
        }
        _ => {}
    }
}

fn scan_utf16_version_string(raw: &[u8], key: &str) -> Option<String> {
    let mut pattern = Vec::with_capacity((key.len() + 1) * 2);
    for u in key.encode_utf16() {
        pattern.extend_from_slice(&u.to_le_bytes());
    }
    pattern.extend_from_slice(&0u16.to_le_bytes());

    let mut search_from = 0;
    while search_from + pattern.len() <= raw.len() {
        let rel = raw[search_from..]
            .windows(pattern.len())
            .position(|w| w == pattern.as_slice())?;
        let key_off = search_from + rel;
        search_from = key_off + 2;

        if key_off < 6 {
            continue;
        }
        let hdr_off = key_off - 6;
        if hdr_off % 4 != 0 {
            continue;
        }
        let Some(w_len) = read_u16_le(raw, hdr_off) else {
            continue;
        };
        let Some(w_val_len) = read_u16_le(raw, hdr_off + 2) else {
            continue;
        };
        if w_len < 8 || w_val_len == 0 {
            continue;
        }
        let block_end = hdr_off.saturating_add(w_len as usize).min(raw.len());
        let val_off = ((key_off + pattern.len()) + 3) & !3;
        if val_off + 2 > block_end {
            continue;
        }

        let mut val_u16 = Vec::new();
        let mut vpos = val_off;
        while vpos + 2 <= block_end {
            let Some(ch) = read_u16_le(raw, vpos) else {
                break;
            };
            vpos += 2;
            if ch == 0 {
                break;
            }
            val_u16.push(ch);
        }
        let s = String::from_utf16_lossy(&val_u16)
            .trim_matches(|c: char| c.is_control() || c.is_whitespace())
            .to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    None
}

fn parse_manifest_description(raw: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(raw)
        .ok()
        .map(|s| s.to_string())
        .unwrap_or_else(|| String::from_utf8_lossy(raw).into_owned());
    let lower = text.to_ascii_lowercase();
    let start_tag = lower.find("<description")?;
    let gt = text[start_tag..].find('>')? + start_tag + 1;
    let end_tag = lower[gt..].find("</description>")? + gt;
    let inner = text[gt..end_tag]
        .replace("&amp;", "&")
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
        .trim()
        .to_string();
    if inner.is_empty() || is_generic_metadata_boilerplate(&inner) {
        None
    } else {
        Some(inner)
    }
}

/// Parse an `IMAGE_RESOURCE_DIRECTORY` at `dir_off` inside `buf`.
/// Returns Vec of `(Option<u32_id>, is_subdir, offset_from_root_rsrc)`.
fn parse_resource_dir(buf: &[u8], dir_off: usize) -> Option<Vec<(Option<u32>, bool, usize)>> {
    if dir_off + 16 > buf.len() {
        return None;
    }
    let num_named = read_u16_le(buf, dir_off + 12)? as usize;
    let num_id = read_u16_le(buf, dir_off + 14)? as usize;
    let total = num_named.checked_add(num_id)?;
    if total > 4096 {
        return None;
    }

    let mut out = Vec::with_capacity(total);
    let entries_start = dir_off + 16;

    for i in 0..total {
        let e_off = entries_start + i * 8;
        if e_off + 8 > buf.len() {
            break;
        }
        let name_or_id = read_u32_le(buf, e_off)?;
        let offset_or_dir = read_u32_le(buf, e_off + 4)?;

        let id = if (name_or_id & 0x8000_0000) == 0 {
            Some(name_or_id & 0xFFFF)
        } else {
            None
        };
        let is_subdir = (offset_or_dir & 0x8000_0000) != 0;
        let offset = (offset_or_dir & 0x7FFF_FFFF) as usize;
        out.push((id, is_subdir, offset));
    }

    Some(out)
}

/// Convert raw `RT_ICON` resource bytes (either PNG or DIB/BMP frame) into a `CandidateIcon`.
fn decode_rt_icon_bytes(raw: &[u8]) -> Option<CandidateIcon> {
    // Case 1: Raw PNG embedded in RT_ICON
    if raw.len() > 24 && &raw[0..8] == b"\x89PNG\r\n\x1a\n" {
        let width = u32::from_be_bytes([raw[16], raw[17], raw[18], raw[19]]);
        let height = u32::from_be_bytes([raw[20], raw[21], raw[22], raw[23]]);
        if width > 0 && height > 0 && width <= 2048 && height <= 2048 {
            return Some(CandidateIcon {
                width,
                height,
                bpp: 32,
                png_bytes: raw.to_vec(),
            });
        }
    }

    // Case 2: Windows DIB BITMAPINFOHEADER (biSize == 40)
    if raw.len() >= 40 {
        let bi_size = read_u32_le(raw, 0)?;
        if bi_size == 40 {
            let width = read_u32_le(raw, 4)?;
            let height_doubled = read_u32_le(raw, 8)?;
            let height = height_doubled / 2;
            let planes = read_u16_le(raw, 12)?;
            let bpp = read_u16_le(raw, 14)?;

            if width == 0 || height == 0 || width > 512 || height > 512 {
                return None;
            }

            // Construct a synthetic 22-byte ICO container around this single DIB entry
            let mut ico_buf = Vec::with_capacity(22 + raw.len());
            // ICONDIR (6 bytes): Reserved=0, Type=1 (Icon), Count=1
            ico_buf.extend_from_slice(&[0, 0, 1, 0, 1, 0]);
            // ICONDIRENTRY (16 bytes)
            let w_byte = if width >= 256 { 0u8 } else { width as u8 };
            let h_byte = if height >= 256 { 0u8 } else { height as u8 };
            ico_buf.push(w_byte);
            ico_buf.push(h_byte);
            ico_buf.push(0); // color count
            ico_buf.push(0); // reserved
            ico_buf.extend_from_slice(&planes.to_le_bytes());
            ico_buf.extend_from_slice(&bpp.to_le_bytes());
            ico_buf.extend_from_slice(&(raw.len() as u32).to_le_bytes());
            ico_buf.extend_from_slice(&22u32.to_le_bytes()); // offset = 22
            ico_buf.extend_from_slice(raw);

            let icon_dir = ico::IconDir::read(Cursor::new(ico_buf)).ok()?;
            let entry = icon_dir.entries().first()?;
            let image = entry.decode().ok()?;
            let mut png_bytes = Vec::new();
            image.write_png(&mut png_bytes).ok()?;

            return Some(CandidateIcon {
                width: image.width(),
                height: image.height(),
                bpp,
                png_bytes,
            });
        }
    }

    None
}

fn read_u16_le(buf: &[u8], offset: usize) -> Option<u16> {
    let slice = buf.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32_le(buf: &[u8], offset: usize) -> Option<u32> {
    let slice = buf.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn push_utf16z(buf: &mut Vec<u8>, s: &str) {
        for u in s.encode_utf16() {
            buf.extend_from_slice(&u.to_le_bytes());
        }
        buf.extend_from_slice(&0u16.to_le_bytes());
    }

    fn align4(buf: &mut Vec<u8>) {
        while buf.len() % 4 != 0 {
            buf.push(0);
        }
    }

    fn build_version_string_node(key: &str, val: &str) -> Vec<u8> {
        let mut out = vec![0u8; 6];
        push_utf16z(&mut out, key);
        align4(&mut out);
        let val_wchars = val.encode_utf16().count() + 1;
        push_utf16z(&mut out, val);
        align4(&mut out);
        let w_len = out.len() as u16;
        out[0..2].copy_from_slice(&w_len.to_le_bytes());
        out[2..4].copy_from_slice(&(val_wchars as u16).to_le_bytes());
        out[4..6].copy_from_slice(&1u16.to_le_bytes());
        out
    }

    fn build_vs_version_info(pairs: &[(&str, &str)]) -> Vec<u8> {
        // StringTable "040904B0"
        let mut st = vec![0u8; 6];
        push_utf16z(&mut st, "040904B0");
        align4(&mut st);
        for (k, v) in pairs {
            align4(&mut st);
            st.extend_from_slice(&build_version_string_node(k, v));
        }
        let st_len = st.len() as u16;
        st[0..2].copy_from_slice(&st_len.to_le_bytes());
        st[4..6].copy_from_slice(&1u16.to_le_bytes());

        // StringFileInfo
        let mut sfi = vec![0u8; 6];
        push_utf16z(&mut sfi, "StringFileInfo");
        align4(&mut sfi);
        sfi.extend_from_slice(&st);
        let sfi_len = sfi.len() as u16;
        sfi[0..2].copy_from_slice(&sfi_len.to_le_bytes());
        sfi[4..6].copy_from_slice(&1u16.to_le_bytes());

        // Root VS_VERSION_INFO with 52-byte VS_FIXEDFILEINFO
        let mut root = vec![0u8; 6];
        push_utf16z(&mut root, "VS_VERSION_INFO");
        align4(&mut root);
        let mut fixed = vec![0u8; 52];
        fixed[0..4].copy_from_slice(&0xFEEF04BDu32.to_le_bytes());
        root.extend_from_slice(&fixed);
        align4(&mut root);
        root.extend_from_slice(&sfi);
        let root_len = root.len() as u16;
        root[0..2].copy_from_slice(&root_len.to_le_bytes());
        root[2..4].copy_from_slice(&52u16.to_le_bytes());
        root[4..6].copy_from_slice(&0u16.to_le_bytes());
        root
    }

    /// Build a minimal valid 64-bit PE `.exe` byte buffer with `.rsrc` containing `RT_VERSION` (Type 16).
    pub(crate) fn build_synthetic_pe_exe(pairs: &[(&str, &str)], manifest_xml: Option<&str>) -> Vec<u8> {
        let ver_data = build_vs_version_info(pairs);
        let manifest_bytes = manifest_xml.map(|s| s.as_bytes().to_vec());
        let has_manifest = manifest_bytes.is_some();

        // Build .rsrc section mapped at RVA 0x2000, file offset 0x400
        let rsrc_rva: u32 = 0x2000;
        let mut rsrc = Vec::new();

        // Layout inside rsrc:
        // 0x00: L1 dir (16 bytes + N*8 entries)
        // If 1 type (RT_VERSION=16):
        //   0x00..0x18: L1 (16 + 8)
        //   0x18..0x30: L2_ver (16 + 8)
        //   0x30..0x48: L3_ver (16 + 8)
        //   0x48..0x58: DataEntry_ver (16)
        //   0x58..: ver_data
        let num_types: u16 = if has_manifest { 2 } else { 1 };
        let l1_size = 16 + (num_types as usize) * 8;
        let l2_ver_off = l1_size;
        let l3_ver_off = l2_ver_off + 24;
        let de_ver_off = l3_ver_off + 24;
        let l2_man_off = de_ver_off + 16;
        let l3_man_off = l2_man_off + if has_manifest { 24 } else { 0 };
        let de_man_off = l3_man_off + if has_manifest { 24 } else { 0 };
        let raw_data_start = ((de_man_off + if has_manifest { 16 } else { 0 }) + 7) & !7;

        let ver_data_off = raw_data_start;
        let man_data_off = ((ver_data_off + ver_data.len()) + 7) & !7;

        let push_dir_1 = |buf: &mut Vec<u8>, id: u32, target_off: usize, is_subdir: bool| {
            buf.extend_from_slice(&[0u8; 12]);
            buf.extend_from_slice(&0u16.to_le_bytes()); // named
            buf.extend_from_slice(&1u16.to_le_bytes()); // id count
            buf.extend_from_slice(&id.to_le_bytes());
            let flag = if is_subdir { 0x8000_0000u32 } else { 0u32 };
            buf.extend_from_slice(&(flag | (target_off as u32)).to_le_bytes());
        };

        // L1
        rsrc.extend_from_slice(&[0u8; 12]);
        rsrc.extend_from_slice(&0u16.to_le_bytes());
        rsrc.extend_from_slice(&num_types.to_le_bytes());
        rsrc.extend_from_slice(&16u32.to_le_bytes()); // RT_VERSION
        rsrc.extend_from_slice(&(0x8000_0000u32 | (l2_ver_off as u32)).to_le_bytes());
        if has_manifest {
            rsrc.extend_from_slice(&24u32.to_le_bytes()); // RT_MANIFEST
            rsrc.extend_from_slice(&(0x8000_0000u32 | (l2_man_off as u32)).to_le_bytes());
        }

        // Version L2 -> L3 -> DataEntry
        push_dir_1(&mut rsrc, 1, l3_ver_off, true);
        push_dir_1(&mut rsrc, 1033, de_ver_off, false);
        rsrc.extend_from_slice(&(rsrc_rva + ver_data_off as u32).to_le_bytes());
        rsrc.extend_from_slice(&(ver_data.len() as u32).to_le_bytes());
        rsrc.extend_from_slice(&[0u8; 8]);

        if let Some(ref mbytes) = manifest_bytes {
            push_dir_1(&mut rsrc, 1, l3_man_off, true);
            push_dir_1(&mut rsrc, 1033, de_man_off, false);
            rsrc.extend_from_slice(&(rsrc_rva + man_data_off as u32).to_le_bytes());
            rsrc.extend_from_slice(&(mbytes.len() as u32).to_le_bytes());
            rsrc.extend_from_slice(&[0u8; 8]);
        }

        while rsrc.len() < ver_data_off {
            rsrc.push(0);
        }
        rsrc.extend_from_slice(&ver_data);

        if let Some(ref mbytes) = manifest_bytes {
            while rsrc.len() < man_data_off {
                rsrc.push(0);
            }
            rsrc.extend_from_slice(mbytes);
        }
        while rsrc.len() % 512 != 0 {
            rsrc.push(0);
        }

        // Build PE64 header at 0x80, 1 section (.rsrc) at file offset 0x200
        let mut pe = vec![0u8; 0x200];
        pe[0..2].copy_from_slice(b"MZ");
        pe[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        let p = 0x80;
        pe[p..p + 4].copy_from_slice(b"PE\0\0");
        pe[p + 4..p + 6].copy_from_slice(&0x8664u16.to_le_bytes()); // AMD64
        pe[p + 6..p + 8].copy_from_slice(&1u16.to_le_bytes()); // 1 section
        let opt_size: u16 = 240;
        pe[p + 20..p + 22].copy_from_slice(&opt_size.to_le_bytes());
        let opt = p + 24;
        pe[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes()); // PE32+
        let rsrc_dd = opt + 112 + 2 * 8;
        pe[rsrc_dd..rsrc_dd + 4].copy_from_slice(&rsrc_rva.to_le_bytes());
        pe[rsrc_dd + 4..rsrc_dd + 8].copy_from_slice(&(rsrc.len() as u32).to_le_bytes());

        let sec = opt + opt_size as usize;
        pe[sec..sec + 5].copy_from_slice(b".rsrc");
        pe[sec + 8..sec + 12].copy_from_slice(&(rsrc.len() as u32).to_le_bytes()); // VirtualSize
        pe[sec + 12..sec + 16].copy_from_slice(&rsrc_rva.to_le_bytes()); // VirtualAddress
        pe[sec + 16..sec + 20].copy_from_slice(&(rsrc.len() as u32).to_le_bytes()); // SizeOfRawData
        pe[sec + 20..sec + 24].copy_from_slice(&0x200u32.to_le_bytes()); // PointerToRawData

        pe.extend_from_slice(&rsrc);
        pe
    }

    #[test]
    fn test_extract_pe_metadata_and_boilerplate_filter() {
        let tmp = std::env::temp_dir().join("doors_test_pe_meta");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let exe_path = tmp.join("setup.exe");
        let pe_bytes = build_synthetic_pe_exe(
            &[
                ("ProductName", "Nebula Odyssey: Horizon"),
                ("FileDescription", "Nebula Odyssey Setup"),
                ("CompanyName", "Starlight Interactive"),
                ("InternalName", "nebula_odyssey"),
                ("OriginalFilename", "NebulaOdyssey.exe"),
            ],
            Some("<assembly><description>Nebula Odyssey Installer</description></assembly>"),
        );
        fs::write(&exe_path, pe_bytes).unwrap();

        let meta = extract_exe_metadata(&exe_path);
        assert_eq!(meta.product_name.as_deref(), Some("Nebula Odyssey: Horizon"));
        assert_eq!(meta.file_description.as_deref(), Some("Nebula Odyssey Setup"));
        assert_eq!(meta.internal_name.as_deref(), Some("nebula_odyssey"));
        assert_eq!(meta.original_filename.as_deref(), Some("NebulaOdyssey.exe"));
        assert_eq!(meta.best_title().as_deref(), Some("Nebula Odyssey: Horizon"));

        // Verify boilerplate installer engine names are rejected cleanly
        let generic_exe = tmp.join("generic_setup.exe");
        let generic_bytes = build_synthetic_pe_exe(
            &[
                ("ProductName", "Inno Setup"),
                ("FileDescription", "Setup/Uninstall"),
            ],
            Some("<assembly><description>Inno Setup</description></assembly>"),
        );
        fs::write(&generic_exe, generic_bytes).unwrap();
        let generic_meta = extract_exe_metadata(&generic_exe);
        assert_eq!(generic_meta.best_title(), None);

        let _ = fs::remove_dir_all(&tmp);
    }
}

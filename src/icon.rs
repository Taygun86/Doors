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

impl CandidateIcon {
    fn score(&self) -> u64 {
        let pixels = (self.width as u64) * (self.height as u64);
        pixels * 100 + (self.bpp as u64)
    }
}

/// Extract the best icon from `exe_path` (or sibling `.ico`/`.exe` files in `game_dir`)
/// and return the raw `.png` bytes in memory.
pub fn extract_exe_icon_png_bytes(exe_path: &Path, game_dir: Option<&Path>) -> Option<Vec<u8>> {
    // 1. Try extracting directly from the main .exe or .msi
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

fn extract_best_icon_from_pe_file(exe_path: &Path) -> Option<CandidateIcon> {
    // Read up to 64 MB or entire file if reasonable; Wait: some game .exe files are 100-300 MB!
    // Instead of reading 300 MB into RAM, let's use File seek/read or memory-efficient section reading!
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

    // Find the section containing `rsrc_rva`
    let rsrc_sec = sections.iter().find(|s| {
        rsrc_rva >= s.virtual_address && rsrc_rva < s.virtual_address.saturating_add(s.virtual_size)
    })?;

    let sec_file_offset = rsrc_sec.raw_data_ptr as u64;
    let sec_read_size = (rsrc_sec.raw_data_size as usize).min(32 * 1024 * 1024); // cap at 32 MB
    if sec_file_offset + (sec_read_size as u64) > file_len {
        return None;
    }

    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(sec_file_offset)).ok()?;
    let mut rsrc_buf = vec![0u8; sec_read_size];
    file.read_exact(&mut rsrc_buf).ok()?;

    let rva_to_buf_offset = |rva: u32| -> Option<usize> {
        if rva >= rsrc_sec.virtual_address {
            let off = (rva - rsrc_sec.virtual_address) as usize;
            if off < rsrc_buf.len() {
                return Some(off);
            }
        }
        None
    };

    let root_off = rva_to_buf_offset(rsrc_rva)?;

    // Level 1: Resource Types (Look for Type ID == 3 which is RT_ICON)
    let l1_entries = parse_resource_dir(&rsrc_buf, root_off)?;
    let mut best_icon: Option<CandidateIcon> = None;

    for (type_id, is_dir_l1, offset_l1) in l1_entries {
        if type_id != Some(3) || !is_dir_l1 {
            continue;
        }
        let l2_off = root_off.checked_add(offset_l1)?;
        let l2_entries = parse_resource_dir(&rsrc_buf, l2_off)?;

        for (_icon_id, is_dir_l2, offset_l2) in l2_entries {
            if !is_dir_l2 {
                continue;
            }
            let l3_off = root_off.checked_add(offset_l2)?;
            let Some(l3_entries) = parse_resource_dir(&rsrc_buf, l3_off) else {
                continue;
            };

            for (_lang_id, is_dir_l3, offset_l3) in l3_entries {
                if is_dir_l3 {
                    continue;
                }
                let data_entry_off = root_off.checked_add(offset_l3)?;
                if data_entry_off + 16 > rsrc_buf.len() {
                    continue;
                }
                let data_rva = read_u32_le(&rsrc_buf, data_entry_off)?;
                let data_size = read_u32_le(&rsrc_buf, data_entry_off + 4)? as usize;

                let Some(icon_offset) = rva_to_buf_offset(data_rva) else {
                    continue;
                };
                if icon_offset + data_size > rsrc_buf.len() || data_size < 16 {
                    continue;
                }

                let raw_icon = &rsrc_buf[icon_offset..icon_offset + data_size];
                if let Some(cand) = decode_rt_icon_bytes(raw_icon) {
                    if best_icon
                        .as_ref()
                        .map_or(true, |b| cand.score() > b.score())
                    {
                        best_icon = Some(cand);
                    }
                }
            }
        }
    }

    best_icon
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

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

static UMU_DATABASE_CSV: &str = include_str!("../data/umu-database.csv");

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct UmuEntry {
    pub title: String,
    pub store: String,
    pub codename: String,
    pub umu_id: String,
    pub acronym: String,
    pub exe_strings: String,
}

static CACHED_ENTRIES: RwLock<Option<Vec<UmuEntry>>> = RwLock::new(None);

pub fn get_cached_database_path() -> PathBuf {
    let home = crate::disks::real_user_home();
    let flatpak_dir = home.join(".var/app/io.github.doors.Doors/data");
    let in_flatpak = Path::new("/.flatpak-info").exists();
    if in_flatpak || flatpak_dir.is_dir() {
        flatpak_dir.join("umu-database.csv")
    } else {
        home.join(".local/share/doors/umu-database.csv")
    }
}

fn parse_entries_from_str(content: &str) -> Vec<UmuEntry> {
    let mut entries = Vec::new();
    for line in content.lines().skip(1) {
        let row = parse_csv_line(line);
        if row.len() >= 4 {
            let title = row[0].trim().to_string();
            let store = row[1].trim().to_string();
            let codename = row[2].trim().to_string();
            let umu_id = row[3].trim().to_string();
            let acronym = row.get(4).map(|s| s.trim().to_string()).unwrap_or_default();
            let exe_strings = row.get(6).map(|s| s.trim().to_string()).unwrap_or_default();

            if !title.is_empty() && !umu_id.is_empty() {
                entries.push(UmuEntry {
                    title,
                    store,
                    codename,
                    umu_id,
                    acronym,
                    exe_strings,
                });
            }
        }
    }
    entries
}

fn get_database() -> Vec<UmuEntry> {
    if let Ok(guard) = CACHED_ENTRIES.read() {
        if let Some(ref db) = *guard {
            return db.clone();
        }
    }

    // Try reading cached file from disk if present
    let cache_path = get_cached_database_path();
    let entries = if cache_path.is_file() {
        if let Ok(content) = fs::read_to_string(&cache_path) {
            let parsed = parse_entries_from_str(&content);
            if !parsed.is_empty() {
                parsed
            } else {
                parse_entries_from_str(UMU_DATABASE_CSV)
            }
        } else {
            parse_entries_from_str(UMU_DATABASE_CSV)
        }
    } else {
        parse_entries_from_str(UMU_DATABASE_CSV)
    };

    if let Ok(mut guard) = CACHED_ENTRIES.write() {
        *guard = Some(entries.clone());
    }

    entries
}

pub fn reload_database() {
    if let Ok(mut guard) = CACHED_ENTRIES.write() {
        *guard = None;
    }
}

/// Asynchronously sync the latest UMU database from GitHub in the background
pub fn sync_umu_database_in_background() {
    std::thread::spawn(|| {
        let cache_path = get_cached_database_path();
        if let Some(parent) = cache_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let url = "https://raw.githubusercontent.com/Open-Wine-Components/umu-database/main/umu-database.csv";
        let mut cmd = crate::proton_manager::build_command("curl");
        cmd.arg("-sSL")
            .arg("--max-time")
            .arg("10")
            .arg("-H")
            .arg("User-Agent: Doors-App")
            .arg(url);

        if let Ok(output) = cmd.output() {
            if output.status.success() && output.stdout.len() > 10000 {
                let content = String::from_utf8_lossy(&output.stdout);
                if content.starts_with("TITLE,STORE") {
                    let _ = fs::write(&cache_path, &output.stdout);
                    reload_database();
                    eprintln!(
                        "Doors: Successfully synced latest UMU database ({} entries)",
                        content.lines().count().saturating_sub(1)
                    );
                }
            }
        }
    });
}

/// Simple CSV line splitter that respects quoted values.
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if in_quotes && chars.peek() == Some(&'"') {
                    current.push('"');
                    chars.next();
                } else {
                    in_quotes = !in_quotes;
                }
            }
            ',' if !in_quotes => {
                fields.push(current.trim().to_string());
                current = String::new();
            }
            _ => {
                current.push(c);
            }
        }
    }
    fields.push(current.trim().to_string());
    fields
}

/// Normalize strings: lowercase and remove non-alphanumeric punctuation.
fn normalize(s: &str) -> String {
    let lower = s.to_lowercase();
    let clean: String = lower
        .chars()
        .map(|c| if c.is_alphanumeric() || c.is_whitespace() { c } else { ' ' })
        .collect();
    clean.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// Standard generic software binary wrappers stripped from executable filenames.
const GENERIC_BINARY_SUFFIXES: &[&str] = &[
    "launcher", "shipping", "retail", "client", "engine", "binaries", "bin",
    "win64", "win32", "x64", "x86", "dx11", "dx12", "vulkan", "game",
];

const STOP_WORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "of", "to", "in", "for", "on", "with", "by", "at",
];

/// Separate letter-to-digit transitions (e.g. "game3" -> "game 3").
fn split_alpha_num(s: &str) -> String {
    let mut out = String::new();
    let mut prev_is_alpha = false;
    for c in s.chars() {
        if c.is_ascii_digit() && prev_is_alpha {
            out.push(' ');
        }
        out.push(c);
        prev_is_alpha = c.is_ascii_alphabetic();
    }
    out
}

/// Extract primary title before subtitle delimiters (`:`, `—`, `+`, ` - `).
fn extract_primary_title(s: &str) -> &str {
    for sep in [":", " — ", " - ", " + "] {
        if let Some((prefix, _)) = s.split_once(sep) {
            let trimmed = prefix.trim();
            if trimmed.len() >= 3 {
                return trimmed;
            }
        }
    }
    s.trim()
}

/// Lookup a game in the UMU database by game name and executable name.
/// Returns Some(UmuEntry) if a valid matching entry is found, otherwise None.
pub fn lookup_umu_game(game_name: &str, exe_name: &str) -> Option<UmuEntry> {
    let raw_exe_stem = exe_name
        .trim_end_matches(".exe")
        .trim_end_matches(".EXE");
    let norm_exe = normalize(raw_exe_stem);
    let split_exe = normalize(&split_alpha_num(raw_exe_stem));

    // Strip generic binary wrappers from exe stem (e.g. "Game Launcher" -> "Game")
    let exe_words: Vec<&str> = split_exe
        .split_whitespace()
        .filter(|w| !GENERIC_BINARY_SUFFIXES.contains(w))
        .collect();
    let clean_exe = exe_words.join(" ");

    let norm_game = normalize(game_name);
    let primary_game = normalize(extract_primary_title(game_name));

    // Priority candidates
    let mut candidates: Vec<(String, f32)> = Vec::new();
    let mut add_cand = |s: String, weight: f32| {
        if s.len() >= 2 && !candidates.iter().any(|(c, _)| c == &s) {
            candidates.push((s, weight));
        }
    };

    if !clean_exe.is_empty() {
        add_cand(clean_exe.clone(), 1.3);
    }
    if !primary_game.is_empty() && primary_game != norm_game {
        add_cand(primary_game, 1.1);
    }
    if !norm_game.is_empty() {
        add_cand(norm_game, 1.0);
    }
    if !norm_exe.is_empty() {
        add_cand(norm_exe, 0.9);
    }
    if !split_exe.is_empty() && split_exe != clean_exe {
        add_cand(split_exe, 0.9);
    }

    if candidates.is_empty() {
        return None;
    }

    let db = get_database();

    // 1. Exact match on full normalized title or primary title
    for (cand, _) in &candidates {
        for entry in &db {
            let norm_t = normalize(&entry.title);
            let norm_p = normalize(extract_primary_title(&entry.title));
            if cand == &norm_t || cand == &norm_p {
                return Some(entry.clone());
            }
        }
    }

    // 2. Acronym and exe_strings match
    for (cand, _) in &candidates {
        for entry in &db {
            let acro = normalize(&entry.acronym);
            if !acro.is_empty() && &acro == cand {
                return Some(entry.clone());
            }
            if !entry.exe_strings.is_empty() {
                for exe_cand in entry.exe_strings.split(';') {
                    let norm_cand = normalize(exe_cand.trim_end_matches(".exe"));
                    if !norm_cand.is_empty() && &norm_cand == cand {
                        return Some(entry.clone());
                    }
                }
            }
        }
    }

    // 3. Contiguous token prefix matching (N-gram prefix)
    let mut best_match: Option<UmuEntry> = None;
    let mut best_score: f32 = 0.0;

    for (cand, weight) in &candidates {
        let c_words: Vec<&str> = cand.split_whitespace().collect();
        if c_words.is_empty() {
            continue;
        }

        for entry in &db {
            let norm_p = normalize(extract_primary_title(&entry.title));
            let p_words: Vec<&str> = norm_p.split_whitespace().collect();
            if p_words.is_empty() {
                continue;
            }

            // Count matching contiguous words from start
            let mut matched_len = 0;
            for (w1, w2) in c_words.iter().zip(p_words.iter()) {
                if w1 == w2 {
                    matched_len += 1;
                } else {
                    break;
                }
            }

            if matched_len >= 2
                || (matched_len == 1 && c_words[0].len() >= 6 && p_words.contains(&c_words[0]))
            {
                let meaningful: Vec<&str> = c_words[..matched_len]
                    .iter()
                    .copied()
                    .filter(|w| !STOP_WORDS.contains(w))
                    .collect();

                if !meaningful.is_empty() {
                    let ratio_p = matched_len as f32 / p_words.len() as f32;
                    let ratio_c = matched_len as f32 / c_words.len() as f32;

                    let mut score =
                        (meaningful.len() as f32 * 12.0 + ratio_p * 8.0 + ratio_c * 4.0) * weight;

                    if matched_len == p_words.len() {
                        score += 25.0;
                    }
                    if meaningful.iter().any(|w| w.chars().any(|c| c.is_ascii_digit())) {
                        score += 15.0;
                    }

                    if score > best_score {
                        best_score = score;
                        best_match = Some(entry.clone());
                    }
                }
            }
        }
    }

    if best_score >= 25.0 {
        return best_match;
    }

    // 4. Online live API fallback query with top candidates
    for (cand, _) in &candidates {
        if let Some(online_match) = query_online_umu_api(cand) {
            return Some(online_match);
        }
    }

    None
}

/// Query the live UMU API for newly added games
fn query_online_umu_api(game_name: &str) -> Option<UmuEntry> {
    let clean = game_name.trim();
    if clean.is_empty() {
        return None;
    }

    let encoded: String = clean
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~' {
                (b as char).to_string()
            } else {
                format!("%{:02X}", b)
            }
        })
        .collect();

    let url = format!("https://umu.openwinecomponents.org/umu_api.php?title={encoded}");
    let mut cmd = crate::proton_manager::build_command("curl");
    cmd.arg("-sSL")
        .arg("--max-time")
        .arg("2")
        .arg("-H")
        .arg("User-Agent: Doors-App")
        .arg(&url);

    if let Ok(output) = cmd.output() {
        if output.status.success() && !output.stdout.is_empty() {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(arr) = val.as_array() {
                    for item in arr {
                        let title = item["title"].as_str().unwrap_or(clean).to_string();
                        let umu_id = item["umu_id"].as_str().unwrap_or("").to_string();
                        let store = item["store"].as_str().unwrap_or("none").to_string();
                        let codename = item["codename"].as_str().unwrap_or("").to_string();
                        if !umu_id.is_empty() {
                            return Some(UmuEntry {
                                title,
                                store,
                                codename,
                                umu_id,
                                acronym: String::new(),
                                exe_strings: String::new(),
                            });
                        }
                    }
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_umu_lookup() {
        let db = get_database();
        assert!(!db.is_empty(), "Database should contain loaded entries");

        // Verify lookup dynamically against entries from the database
        let first = &db[0];
        let found = lookup_umu_game(&first.title, "game.exe");
        assert!(found.is_some());
        assert_eq!(found.unwrap().umu_id, first.umu_id);

        if db.len() > 10 {
            let sample = &db[10];
            let found_sample = lookup_umu_game(&sample.title, "launcher.exe");
            assert!(found_sample.is_some());
            assert_eq!(found_sample.unwrap().umu_id, sample.umu_id);
        }

        // Test non-existent application
        let unknown = lookup_umu_game("SampleNonExistentApp 9999", "unregistered_test_app.exe");
        assert!(unknown.is_none());
    }
}

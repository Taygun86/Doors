use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;

static UMU_DATABASE_CSV: &str = include_str!("../data/umu-database.csv");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UmuEntry {
    pub title: String,
    pub store: String,
    pub codename: String,
    pub umu_id: String,
    pub acronym: String,
    pub exe_strings: String,
}

struct TargetKey {
    norm: String,
    spaceless: String,
    words: Vec<String>,
    meaningful_count: usize,
    base_score: f32,
}

struct IndexedUmuEntry {
    entry: UmuEntry,
    pref_bonus: f32,
    targets: Vec<TargetKey>,
    norm_acronym: String,
    norm_exes: Vec<String>,
}

struct UmuDatabaseIndex {
    entries: Vec<IndexedUmuEntry>,
    by_token: HashMap<String, Vec<usize>>,
    by_spaceless: HashMap<String, Vec<usize>>,
}

static CACHED_INDEX: RwLock<Option<UmuDatabaseIndex>> = RwLock::new(None);
static UMU_SYNC_RUNNING: AtomicBool = AtomicBool::new(false);

pub fn get_cached_database_path() -> PathBuf {
    let home = crate::disks::real_user_home();
    let flatpak_dir = home.join(format!(".var/app/{}/data", crate::desktop::APP_ID));
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
        if line.trim().is_empty() {
            continue;
        }
        if !line.contains('"') {
            let mut cols = line.split(',');
            let Some(t) = cols.next() else { continue };
            let Some(st) = cols.next() else { continue };
            let Some(cn) = cols.next() else { continue };
            let Some(uid) = cols.next() else { continue };
            let acro = cols.next().unwrap_or("").trim();
            let _note = cols.next();
            let exes = cols.next().unwrap_or("").trim();

            let title = t.trim();
            let umu_id = uid.trim();
            if !title.is_empty() && !umu_id.is_empty() {
                entries.push(UmuEntry {
                    title: title.to_string(),
                    store: st.trim().to_string(),
                    codename: cn.trim().to_string(),
                    umu_id: umu_id.to_string(),
                    acronym: acro.to_string(),
                    exe_strings: exes.to_string(),
                });
            }
        } else {
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
    }
    entries
}

impl UmuDatabaseIndex {
    fn from_entries(raw_entries: Vec<UmuEntry>) -> Self {
        let mut entries = Vec::with_capacity(raw_entries.len());
        let mut by_token: HashMap<String, Vec<usize>> = HashMap::with_capacity(raw_entries.len() * 2);
        let mut by_spaceless: HashMap<String, Vec<usize>> = HashMap::with_capacity(raw_entries.len());

        let push_idx = |map: &mut HashMap<String, Vec<usize>>, key: &str, idx: usize| {
            let vec = map.entry(key.to_string()).or_default();
            if vec.last().copied() != Some(idx) {
                vec.push(idx);
            }
        };

        for (idx, entry) in raw_entries.into_iter().enumerate() {
            let pref_bonus = entry_preference_bonus(&entry);
            let norm_full = normalize(&entry.title);
            let norm_full_base = strip_edition_suffix(&norm_full);
            let norm_primary = strip_edition_suffix(&normalize(extract_primary_title(&entry.title)));
            let subtitle_opt = extract_meaningful_subtitle(&entry.title);
            let slug_opt = extract_umu_id_slug(&entry.umu_id);
            let raw_slug_opt = entry
                .umu_id
                .strip_prefix("umu-")
                .filter(|s| s.len() >= 4 && !s.chars().all(|c| c.is_ascii_digit()))
                .filter(|s| !(s.len() >= 24 && s.chars().all(|c| c.is_ascii_hexdigit())))
                .map(normalize);

            let codename_slug = if !entry.store.eq_ignore_ascii_case("egs")
                && !entry.codename.eq_ignore_ascii_case("none")
                && entry.codename.len() >= 5
                && !entry.codename.chars().all(|c| c.is_ascii_digit())
                && !entry.codename.contains('-')
                && !(entry.codename.len() >= 24 && entry.codename.chars().all(|c| c.is_ascii_hexdigit()))
            {
                Some(normalize(&entry.codename))
            } else {
                None
            };

            let mut raw_targets: Vec<(String, f32)> = Vec::with_capacity(6);
            let mut add_t = |s: String, score: f32| {
                if !s.is_empty() && !raw_targets.iter().any(|(k, _)| k == &s) {
                    raw_targets.push((s, score));
                }
            };

            add_t(norm_full, 100.0);
            add_t(norm_full_base, 96.0);
            add_t(norm_primary, 92.0);
            if let Some(slug) = slug_opt {
                add_t(slug, 94.0);
            }
            if let Some(rslug) = raw_slug_opt {
                add_t(rslug, 94.0);
            }
            if let Some(sub) = subtitle_opt {
                add_t(sub, 88.0);
            }
            if let Some(cslug) = codename_slug {
                add_t(cslug, 86.0);
            }

            let mut targets = Vec::with_capacity(raw_targets.len());
            for (norm, base_score) in raw_targets {
                let words: Vec<String> = norm.split_whitespace().map(|w| w.to_string()).collect();
                if words.is_empty() {
                    continue;
                }
                let spaceless: String = words.concat();
                let meaningful_count = words
                    .iter()
                    .filter(|w| {
                        let ws = w.as_str();
                        !STOP_WORDS.contains(&ws) && !GENERIC_BINARY_TOKENS.contains(&ws)
                    })
                    .count();

                push_idx(&mut by_token, &words[0], idx);
                if spaceless.len() >= 6 {
                    push_idx(&mut by_spaceless, &spaceless, idx);
                }

                targets.push(TargetKey {
                    norm,
                    spaceless,
                    words,
                    meaningful_count,
                    base_score,
                });
            }

            let norm_acronym = if entry.acronym.is_empty() {
                String::new()
            } else {
                let a = normalize(&entry.acronym);
                if let Some(first_w) = a.split_whitespace().next() {
                    if a.len() >= 3 {
                        push_idx(&mut by_token, first_w, idx);
                    }
                }
                a
            };

            let mut norm_exes = Vec::new();
            if !entry.exe_strings.is_empty() {
                for exe_cand in entry.exe_strings.split(';') {
                    let ne = normalize(
                        exe_cand
                            .trim()
                            .trim_end_matches(".exe")
                            .trim_end_matches(".EXE"),
                    );
                    if !ne.is_empty() {
                        if let Some(first_w) = ne.split_whitespace().next() {
                            push_idx(&mut by_token, first_w, idx);
                        }
                        norm_exes.push(ne);
                    }
                }
            }

            entries.push(IndexedUmuEntry {
                entry,
                pref_bonus,
                targets,
                norm_acronym,
                norm_exes,
            });
        }

        Self {
            entries,
            by_token,
            by_spaceless,
        }
    }
}

fn with_database_index<R>(f: impl FnOnce(&UmuDatabaseIndex) -> R) -> R {
    if let Ok(guard) = CACHED_INDEX.read() {
        if let Some(ref db) = *guard {
            return f(db);
        }
    }

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

    let built = UmuDatabaseIndex::from_entries(entries);
    if let Ok(mut guard) = CACHED_INDEX.write() {
        *guard = Some(built);
    }

    if let Ok(guard) = CACHED_INDEX.read() {
        if let Some(ref db) = *guard {
            return f(db);
        }
    }

    let fallback = UmuDatabaseIndex::from_entries(parse_entries_from_str(UMU_DATABASE_CSV));
    f(&fallback)
}

pub fn reload_database() {
    if let Ok(mut guard) = CACHED_INDEX.write() {
        *guard = None;
    }
}

/// Synchronously download and refresh the latest UMU database cache from GitHub.
pub fn sync_umu_database_blocking() {
    if UMU_SYNC_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    struct SyncGuard;
    impl Drop for SyncGuard {
        fn drop(&mut self) {
            UMU_SYNC_RUNNING.store(false, Ordering::SeqCst);
        }
    }
    let _guard = SyncGuard;

    let cache_path = get_cached_database_path();
    if let Some(parent) = cache_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let url =
        "https://raw.githubusercontent.com/Open-Wine-Components/umu-database/main/umu-database.csv";
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
}

/// Asynchronously sync the latest UMU database from GitHub in the background.
pub fn sync_umu_database_in_background() {
    std::thread::spawn(sync_umu_database_blocking);
}

/// Clean a UMU database title for display (strips trademark symbols and pack bundle suffixes).
pub fn clean_umu_display_title(raw_title: &str) -> String {
    let mut s = raw_title
        .replace('™', "")
        .replace('®', "")
        .replace('©', "")
        .trim()
        .to_string();

    if let Some((prefix, suffix)) = s.split_once(" + ") {
        let p_trim = prefix.trim();
        let s_lower = suffix.trim().to_lowercase();
        if p_trim.len() >= 3 && (s_lower.ends_with("packs") || s_lower.ends_with("pack")) {
            s = p_trim.to_string();
        }
    }

    s.split_whitespace().collect::<Vec<_>>().join(" ")
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

/// Normalize strings: lowercase and replace non-alphanumeric punctuation with spaces.
fn normalize(s: &str) -> String {
    let lower = s.to_lowercase();
    let clean: String = lower
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect();
    clean.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// Split `CamelCase` and letter-to-digit transitions (e.g. `"SpaceQuest2"` -> `"Space Quest 2"`).
fn split_identifier_words(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 8);

    for i in 0..chars.len() {
        let c = chars[i];
        if i > 0 {
            let prev = chars[i - 1];
            let next_is_lower = chars
                .get(i + 1)
                .map(|n| n.is_ascii_lowercase())
                .unwrap_or(false);

            let camel_boundary = (prev.is_ascii_lowercase() && c.is_ascii_uppercase())
                || (prev.is_ascii_uppercase() && c.is_ascii_uppercase() && next_is_lower);
            let alpha_digit_boundary = prev.is_ascii_alphabetic() && c.is_ascii_digit();

            if camel_boundary || alpha_digit_boundary {
                out.push(' ');
            }
        }
        out.push(c);
    }
    out
}

/// Standard generic software binary wrappers stripped from executable filenames.
const GENERIC_BINARY_TOKENS: &[&str] = &[
    "launcher",
    "shipping",
    "retail",
    "client",
    "engine",
    "binaries",
    "bin",
    "win64",
    "win32",
    "x64",
    "x86",
    "amd64",
    "dx11",
    "dx12",
    "vulkan",
    "game",
    "setup",
    "install",
    "installer",
    "standalone",
    "portable",
    "release",
];

const STOP_WORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "of", "to", "in", "for", "on", "with", "by", "at",
];

const EDITION_SUFFIXES: &[&str] = &[
    "game of the year edition",
    "game of the year",
    "goty edition",
    "goty",
    "definitive edition",
    "enhanced edition",
    "complete edition",
    "special edition",
    "anniversary edition",
    "ultimate edition",
    "collector s edition",
    "collectors edition",
    "directors cut",
    "director s cut",
    "final cut",
    "gold edition",
    "gold",
    "epic edition",
    "remastered",
    "hd",
];

fn strip_edition_suffix(norm: &str) -> String {
    let mut cur = norm.trim().to_string();
    loop {
        let mut changed = false;
        for suf in EDITION_SUFFIXES {
            if let Some(prefix) = cur.strip_suffix(suf) {
                let trimmed = prefix.trim();
                if trimmed.len() >= 3 {
                    cur = trimmed.to_string();
                    changed = true;
                    break;
                }
            }
        }
        if !changed {
            break;
        }
    }
    cur
}

fn is_generic_edition_only(norm: &str) -> bool {
    let t = norm.trim();
    t.is_empty() || EDITION_SUFFIXES.contains(&t)
}

/// Extract primary title before subtitle delimiters (`:`, ` — `, ` - `, ` + `).
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

/// Extract distinct subtitle after `:` if meaningful (not just an edition label).
fn extract_meaningful_subtitle(s: &str) -> Option<String> {
    let (_, raw_sub) = s.split_once(':')?;
    let norm_sub = normalize(raw_sub);
    if is_generic_edition_only(&norm_sub) {
        return None;
    }
    let stripped = strip_edition_suffix(&norm_sub);
    if stripped.len() >= 5
        && !is_generic_edition_only(&stripped)
        && !STOP_WORDS.contains(&stripped.as_str())
    {
        Some(stripped)
    } else {
        None
    }
}

/// Extract human-readable standalone slug from `umu-<slug>` (ignoring numeric Steam AppIDs and hex UUIDs).
fn extract_umu_id_slug(umu_id: &str) -> Option<String> {
    let slug = umu_id.strip_prefix("umu-")?;
    if slug.len() < 4 {
        return None;
    }
    if slug.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if slug.len() >= 24 && slug.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let norm = normalize(&split_identifier_words(slug));
    if norm.len() >= 4 {
        Some(norm)
    } else {
        None
    }
}

fn entry_preference_bonus(entry: &UmuEntry) -> f32 {
    let mut bonus = 0.0;
    let id_tail = entry.umu_id.strip_prefix("umu-").unwrap_or(&entry.umu_id);
    let is_hex_uuid = id_tail.len() >= 24 && id_tail.chars().all(|c| c.is_ascii_hexdigit());
    if !is_hex_uuid {
        bonus += 6.0;
    }
    if entry.store.eq_ignore_ascii_case("none") && entry.codename.eq_ignore_ascii_case("none") {
        bonus += 4.0;
    } else if entry.store.eq_ignore_ascii_case("none") {
        bonus += 3.0;
    } else if entry.store.eq_ignore_ascii_case("steam") || entry.store.eq_ignore_ascii_case("gog") {
        bonus += 1.5;
    }
    bonus
}

fn build_candidates_from_signals(signals: &[&str]) -> Vec<(String, f32)> {
    let mut candidates: Vec<(String, f32)> = Vec::new();
    let mut add_cand = |s: String, weight: f32| {
        let trimmed = s.trim().to_string();
        if trimmed.len() >= 2 {
            if let Some(existing) = candidates.iter_mut().find(|(c, _)| c == &trimmed) {
                if weight > existing.1 {
                    existing.1 = weight;
                }
            } else {
                candidates.push((trimmed, weight));
            }
        }
    };

    for (idx, &raw_sig) in signals.iter().enumerate() {
        let base_weight = (1.35 - (idx as f32 * 0.08)).max(0.85);
        let stem = raw_sig
            .trim()
            .trim_end_matches(".exe")
            .trim_end_matches(".EXE")
            .trim_end_matches(".msi")
            .trim_end_matches(".MSI");
        if stem.is_empty() {
            continue;
        }

        let cleaned_title = crate::detector::clean_game_title(stem);
        let norm_clean = normalize(&cleaned_title);
        let split_clean = normalize(&split_identifier_words(&cleaned_title));

        let filtered_words: Vec<&str> = split_clean
            .split_whitespace()
            .filter(|w| !GENERIC_BINARY_TOKENS.contains(w))
            .collect();
        let stripped_wrappers = filtered_words.join(" ");

        if !stripped_wrappers.is_empty() {
            add_cand(stripped_wrappers.clone(), base_weight);
        }
        if !split_clean.is_empty() {
            add_cand(split_clean, base_weight * 0.96);
        }
        if !norm_clean.is_empty() {
            add_cand(norm_clean.clone(), base_weight * 0.95);
        }

        let primary = normalize(extract_primary_title(&cleaned_title));
        if !primary.is_empty() && primary != norm_clean {
            add_cand(primary, base_weight * 0.94);
        }
    }

    candidates
}

fn match_index_against_candidates(
    db: &UmuDatabaseIndex,
    candidates: &[(String, f32)],
) -> Option<UmuEntry> {
    if candidates.is_empty() || db.entries.is_empty() {
        return None;
    }

    // Collect only entries sharing at least one anchor token or spaceless key with candidates
    let mut candidate_entry_ids = HashSet::new();
    for (cand, _) in candidates {
        for w in cand.split_whitespace() {
            if let Some(ids) = db.by_token.get(w) {
                candidate_entry_ids.extend(ids.iter().copied());
            }
        }
        let spaceless: String = cand.chars().filter(|c| !c.is_whitespace()).collect();
        if spaceless.len() >= 6 {
            if let Some(ids) = db.by_spaceless.get(&spaceless) {
                candidate_entry_ids.extend(ids.iter().copied());
            }
        }
    }

    if candidate_entry_ids.is_empty() {
        return None;
    }

    let mut sorted_ids: Vec<usize> = candidate_entry_ids.into_iter().collect();
    sorted_ids.sort_unstable();

    let mut best_entry: Option<&UmuEntry> = None;
    let mut best_score: f32 = 0.0;

    for idx in sorted_ids {
        let indexed = &db.entries[idx];
        let pref = indexed.pref_bonus;

        for (cand, c_weight) in candidates {
            let cand_spaceless: String = cand.chars().filter(|c| !c.is_whitespace()).collect();
            let c_words: Vec<&str> = cand.split_whitespace().collect();

            // 1. Exact, spaceless-exact, and contiguous subsequence matching
            for t in &indexed.targets {
                if cand == &t.norm {
                    let score = t.base_score * c_weight + pref;
                    if score > best_score {
                        best_score = score;
                        best_entry = Some(&indexed.entry);
                    }
                    continue;
                }

                if cand_spaceless.len() >= 6 && cand_spaceless == t.spaceless {
                    let score = (t.base_score - 3.0) * c_weight + pref;
                    if score > best_score {
                        best_score = score;
                        best_entry = Some(&indexed.entry);
                    }
                    continue;
                }

                if t.words.is_empty()
                    || t.words.len() > c_words.len()
                    || t.meaningful_count == 0
                    || (t.words.len() == 1 && t.words[0].len() < 6)
                {
                    continue;
                }

                for start_idx in 0..=(c_words.len() - t.words.len()) {
                    let slice_matches = c_words[start_idx..start_idx + t.words.len()]
                        .iter()
                        .zip(t.words.iter())
                        .all(|(cw, tw)| *cw == tw.as_str());
                    if slice_matches {
                        let coverage = t.words.len() as f32 / c_words.len() as f32;
                        let pos_bonus = if start_idx == 0 { 4.0 } else { 0.0 };
                        let score = (55.0
                            + t.meaningful_count as f32 * 10.0
                            + coverage * 15.0
                            + pos_bonus)
                            * c_weight
                            + pref;
                        if score > best_score {
                            best_score = score;
                            best_entry = Some(&indexed.entry);
                        }
                        break;
                    }
                }
            }

            // 2. Acronym & exe_strings exact match
            if indexed.norm_acronym.len() >= 3 && &indexed.norm_acronym == cand {
                let score = 82.0 * c_weight + pref;
                if score > best_score {
                    best_score = score;
                    best_entry = Some(&indexed.entry);
                }
            }

            for norm_exe_cand in &indexed.norm_exes {
                if norm_exe_cand == cand {
                    let score = 98.0 * c_weight + pref;
                    if score > best_score {
                        best_score = score;
                        best_entry = Some(&indexed.entry);
                    }
                }
            }
        }
    }

    if best_score >= 50.0 {
        best_entry.cloned()
    } else {
        None
    }
}

/// Multi-signal UMU database lookup (combines PE metadata, folder/game title, and executable filename).
pub fn lookup_umu_with_signals(signals: &[&str]) -> Option<UmuEntry> {
    let candidates = build_candidates_from_signals(signals);
    if candidates.is_empty() {
        return None;
    }

    if let Some(found) = with_database_index(|db| match_index_against_candidates(db, &candidates)) {
        return Some(found);
    }

    // Online live API fallback (at most 1 fast validated query; disabled in unit tests)
    #[cfg(not(test))]
    {
        if let Some((best_cand, _)) = candidates.iter().find(|(c, _)| c.len() >= 5) {
            if let Some(online_entries) = query_online_umu_api(best_cand) {
                let temp_index = UmuDatabaseIndex::from_entries(online_entries);
                if let Some(validated) = match_index_against_candidates(&temp_index, &candidates) {
                    return Some(validated);
                }
            }
        }
    }

    None
}

/// Lookup a game in the UMU database by game name and executable name/path.
#[allow(dead_code)]
pub fn lookup_umu_game(game_name: &str, exe_name: &str) -> Option<UmuEntry> {
    lookup_umu_with_signals(&[game_name, exe_name])
}

/// Lookup a game in the UMU database inspecting embedded PE metadata of `exe_path` + `game_name`.
pub fn lookup_umu_for_exe(game_name: &str, exe_path: &Path) -> Option<UmuEntry> {
    let meta = crate::icon::extract_exe_metadata(exe_path);
    let meta_signals = meta.candidate_signals();
    let exe_fname = exe_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    let mut sig_refs: Vec<&str> = Vec::with_capacity(meta_signals.len() + 2);
    for ms in &meta_signals {
        sig_refs.push(ms.as_str());
    }
    if !game_name.trim().is_empty() {
        sig_refs.push(game_name);
    }
    if !exe_fname.is_empty() {
        sig_refs.push(exe_fname);
    }

    lookup_umu_with_signals(&sig_refs)
}

#[cfg(not(test))]
fn query_online_umu_api(query_str: &str) -> Option<Vec<UmuEntry>> {
    let clean = query_str.trim();
    if clean.len() < 5 {
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

    let output = cmd.output().ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let val = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    let arr = val.as_array()?;

    let mut results = Vec::new();
    for item in arr {
        let title = item["title"].as_str().unwrap_or("").trim().to_string();
        let umu_id = item["umu_id"].as_str().unwrap_or("").trim().to_string();
        let store = item["store"].as_str().unwrap_or("none").trim().to_string();
        let codename = item["codename"].as_str().unwrap_or("none").trim().to_string();
        let acronym = item["acronym"].as_str().unwrap_or("").trim().to_string();
        let exe_strings = item["exe_string"].as_str().unwrap_or("").trim().to_string();

        if !title.is_empty() && !umu_id.is_empty() {
            results.push(UmuEntry {
                title,
                store,
                codename,
                umu_id,
                acronym,
                exe_strings,
            });
        }
    }

    if results.is_empty() {
        None
    } else {
        Some(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_umu_lookup_and_standalone_slugs() {
        let first_title = with_database_index(|db| {
            assert!(!db.entries.is_empty(), "Database should contain loaded entries");
            db.entries[0].entry.title.clone()
        });

        let found = lookup_umu_game(&first_title, "game.exe");
        assert!(found.is_some());
        assert_eq!(found.unwrap().title, first_title);

        let unknown = lookup_umu_game("SampleNonExistentApp 9999", "unregistered_test_app.exe");
        assert!(unknown.is_none());
    }

    #[test]
    fn test_synthetic_launcher_prefix_subtitle_and_camel_case() {
        let csv = "\
TITLE,STORE,CODENAME,UMU_ID,COMMON ACRONYM (Optional),NOTE (Optional),EXE_STRINGS (Optional)
Chronicles of Aether: Sunfall,egs,0123456789abcdef0123456789abcdef,umu-0123456789abcdef0123456789abcdef,,,
Chronicles of Aether: Sunfall,none,none,umu-sunfall,,Standalone PC installer,
Starlight Builder 2 + Starlight Builder 2 Packs,steam,none,umu-990011,,,
Nebula Quest,gog,1234567890,umu-880022,,,
";
        let entries = parse_entries_from_str(csv);
        assert_eq!(entries.len(), 4);
        let index = UmuDatabaseIndex::from_entries(entries);

        // 1. Publisher launcher + version numbers + standalone umu-<slug> / subtitle
        let cands = build_candidates_from_signals(&[
            "StudioLauncher v1 6 0 1607 6 6 sunfall",
            "StudioLauncher_v1.6.0.1607_6_6_sunfall.exe",
        ]);
        let matched = match_index_against_candidates(&index, &cands).unwrap();
        assert_eq!(matched.umu_id, "umu-sunfall");
        assert_eq!(matched.store, "none");

        // 2. Pack bundle title matching edition folder + launcher exe
        let cands_packs = build_candidates_from_signals(&[
            "Starlight Builder 2 LC",
            "Starlight Builder 2 Launcher.exe",
        ]);
        let matched_packs = match_index_against_candidates(&index, &cands_packs).unwrap();
        assert_eq!(matched_packs.umu_id, "umu-990011");
        assert_eq!(
            clean_umu_display_title(&matched_packs.title),
            "Starlight Builder 2"
        );

        // 3. CamelCase single-word executable matching multi-word entry
        let cands_camel = build_candidates_from_signals(&["setup", "NebulaQuest.exe"]);
        let matched_camel = match_index_against_candidates(&index, &cands_camel).unwrap();
        assert_eq!(matched_camel.umu_id, "umu-880022");

        // 4. Short shared word ("Starlight") alone in an unrelated title must NOT false-positive match "Starlight Builder 2"
        let cands_unmatched =
            build_candidates_from_signals(&["Starlight Racing", "StarlightRacing.exe"]);
        assert!(match_index_against_candidates(&index, &cands_unmatched).is_none());
    }
}

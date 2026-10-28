use crate::disks::real_user_home;
use gettext::Catalog;
use once_cell::sync::Lazy;
use std::fs::File;
use std::io::Cursor;
use std::path::PathBuf;

static EMBEDDED_CATALOGS: &[(&str, &[u8])] = &[
    ("ar", include_bytes!("../locale/ar/LC_MESSAGES/doors.mo")),
    ("az", include_bytes!("../locale/az/LC_MESSAGES/doors.mo")),
    ("cs", include_bytes!("../locale/cs/LC_MESSAGES/doors.mo")),
    ("de", include_bytes!("../locale/de/LC_MESSAGES/doors.mo")),
    ("el", include_bytes!("../locale/el/LC_MESSAGES/doors.mo")),
    ("es", include_bytes!("../locale/es/LC_MESSAGES/doors.mo")),
    ("fa", include_bytes!("../locale/fa/LC_MESSAGES/doors.mo")),
    ("fr", include_bytes!("../locale/fr/LC_MESSAGES/doors.mo")),
    ("hi", include_bytes!("../locale/hi/LC_MESSAGES/doors.mo")),
    ("hu", include_bytes!("../locale/hu/LC_MESSAGES/doors.mo")),
    ("id", include_bytes!("../locale/id/LC_MESSAGES/doors.mo")),
    ("it", include_bytes!("../locale/it/LC_MESSAGES/doors.mo")),
    ("ja", include_bytes!("../locale/ja/LC_MESSAGES/doors.mo")),
    ("kk", include_bytes!("../locale/kk/LC_MESSAGES/doors.mo")),
    ("ko", include_bytes!("../locale/ko/LC_MESSAGES/doors.mo")),
    ("nl", include_bytes!("../locale/nl/LC_MESSAGES/doors.mo")),
    ("pl", include_bytes!("../locale/pl/LC_MESSAGES/doors.mo")),
    ("pt", include_bytes!("../locale/pt/LC_MESSAGES/doors.mo")),
    ("ro", include_bytes!("../locale/ro/LC_MESSAGES/doors.mo")),
    ("ru", include_bytes!("../locale/ru/LC_MESSAGES/doors.mo")),
    ("sv", include_bytes!("../locale/sv/LC_MESSAGES/doors.mo")),
    ("tr", include_bytes!("../locale/tr/LC_MESSAGES/doors.mo")),
    ("uk", include_bytes!("../locale/uk/LC_MESSAGES/doors.mo")),
    ("vi", include_bytes!("../locale/vi/LC_MESSAGES/doors.mo")),
    ("zh", include_bytes!("../locale/zh/LC_MESSAGES/doors.mo")),
];

static CATALOG: Lazy<Option<Catalog>> = Lazy::new(|| {
    let lang = detect_language();
    if lang == "en" || lang == "c" || lang == "posix" {
        return None;
    }

    let home = real_user_home();
    let search_bases = [
        PathBuf::from("locale"),
        PathBuf::from("/app/share/locale"),
        home.join(".local/share/locale"),
        PathBuf::from("/usr/local/share/locale"),
        PathBuf::from("/usr/share/locale"),
    ];

    for base in &search_bases {
        let mo_path = base.join(&lang).join("LC_MESSAGES/doors.mo");
        if let Ok(file) = File::open(&mo_path) {
            if let Ok(cat) = Catalog::parse(file) {
                return Some(cat);
            }
        }
    }

    for (code, bytes) in EMBEDDED_CATALOGS {
        if *code == lang {
            return Catalog::parse(Cursor::new(*bytes)).ok();
        }
    }

    None
});

fn detect_language() -> String {
    for var in ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"] {
        if let Ok(val) = std::env::var(var) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return normalize_lang_code(trimmed);
            }
        }
    }
    if let Some(loc) = sys_locale::get_locale() {
        return normalize_lang_code(&loc);
    }
    "en".to_string()
}

fn normalize_lang_code(raw: &str) -> String {
    let first = raw.split(':').next().unwrap_or(raw);
    let without_encoding = first.split('.').next().unwrap_or(first);
    let primary = without_encoding
        .split(['_', '-'])
        .next()
        .unwrap_or(without_encoding);
    primary.to_ascii_lowercase()
}

pub fn tr(msgid: &str) -> String {
    if let Some(ref cat) = *CATALOG {
        cat.gettext(msgid).to_string()
    } else {
        msgid.to_string()
    }
}

//! i18n groundwork (3d): a translation table keyed by the English source
//! string — the same shape gettext extraction produces, so migrating these
//! files to real `.po` catalogues later is mechanical.
//!
//! Locale files live in `<repo>/etc/i18n/<lang>.conf` as `english = translated`
//! lines (`#` comments allowed). Language selection: `SONIC_OXIDE_LANG`, else
//! the `LANG` prefix (`de_DE.UTF-8` → `de`), else English. A missing file or
//! key falls back to the English literal — translations can land
//! incrementally.

use std::collections::HashMap;
use std::path::Path;

pub struct I18n {
    map: HashMap<String, String>,
}

impl I18n {
    /// Detect the user's language: SONIC_OXIDE_LANG wins, else LANG's prefix.
    pub fn detect_lang() -> String {
        if let Ok(lang) = std::env::var("SONIC_OXIDE_LANG") {
            if !lang.is_empty() {
                return lang;
            }
        }
        std::env::var("LANG")
            .ok()
            .and_then(|l| l.split(['_', '.']).next().map(str::to_string))
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| "en".to_string())
    }

    /// Load `<dir>/<lang>.conf`; English (or anything unreadable) yields the
    /// identity table.
    pub fn load(dir: &Path, lang: &str) -> I18n {
        if lang == "en" {
            return I18n { map: HashMap::new() };
        }
        let text = std::fs::read_to_string(dir.join(format!("{lang}.conf"))).unwrap_or_default();
        I18n { map: parse_table(&text) }
    }

    /// Translate, falling back to the English source string.
    pub fn tr<'a>(&'a self, english: &'a str) -> &'a str {
        self.map.get(english).map(String::as_str).unwrap_or(english)
    }
}

fn parse_table(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let (k, v) = (k.trim(), v.trim());
            if !k.is_empty() && !v.is_empty() {
                map.insert(k.to_string(), v.to_string());
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_with_english_fallback() {
        let dir = std::env::temp_dir().join("sonic_oxide_i18n_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("de.conf"), "# demo\nSettings = Einstellungen\nHelp = Hilfe\n")
            .unwrap();

        let de = I18n::load(&dir, "de");
        assert_eq!(de.tr("Settings"), "Einstellungen");
        assert_eq!(de.tr("Help"), "Hilfe");
        assert_eq!(de.tr("Log"), "Log"); // untranslated key → English

        let fr = I18n::load(&dir, "fr"); // no file → identity
        assert_eq!(fr.tr("Settings"), "Settings");
        let en = I18n::load(&dir, "en");
        assert_eq!(en.tr("Settings"), "Settings");
    }

    #[test]
    fn parses_table_tolerantly() {
        let map = parse_table("# c\n\nA = B\nbad line\nX =\n = y\n K = V ");
        assert_eq!(map.len(), 2);
        assert_eq!(map["A"], "B");
        assert_eq!(map["K"], "V");
    }

    #[test]
    fn detects_language_with_the_documented_precedence() {
        // One test mutates env sequentially — no parallel readers of these
        // vars exist elsewhere in this test binary.
        // SAFETY: test-only, sequential env access in this binary.
        unsafe { std::env::set_var("SONIC_OXIDE_LANG", "pt"); }
        // SAFETY: test-only, sequential env access in this binary.
        unsafe { std::env::set_var("LANG", "de_DE.UTF-8"); }
        assert_eq!(I18n::detect_lang(), "pt"); // explicit override wins

        // SAFETY: test-only, sequential env access in this binary.

        unsafe { std::env::remove_var("SONIC_OXIDE_LANG"); }
        assert_eq!(I18n::detect_lang(), "de"); // LANG prefix

        // SAFETY: test-only, sequential env access in this binary.

        unsafe { std::env::set_var("LANG", "fr"); }
        assert_eq!(I18n::detect_lang(), "fr"); // bare LANG

        // SAFETY: test-only, sequential env access in this binary.

        unsafe { std::env::set_var("SONIC_OXIDE_LANG", ""); }
        assert_eq!(I18n::detect_lang(), "fr"); // empty override falls through

        // SAFETY: test-only, sequential env access in this binary.

        unsafe { std::env::remove_var("SONIC_OXIDE_LANG"); }
        // SAFETY: test-only, sequential env access in this binary.
        unsafe { std::env::remove_var("LANG"); }
        assert_eq!(I18n::detect_lang(), "en"); // nothing set → English

        // SAFETY: test-only, sequential env access in this binary.
        unsafe { std::env::set_var("LANG", "en_GB.UTF-8") } // leave sane for other code
    }
}

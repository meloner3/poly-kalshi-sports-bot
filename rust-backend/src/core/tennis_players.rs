//! Tennis player name normalization
//! 网球选手姓名标准化
//!
//! Provides canonical surname keys for consistent matching between
//! platforms, following the `nba_teams.rs` pattern. Tennis is the natural
//! next sport for the matcher: Kalshi lists ATP/WTA match markets daily
//! (series `KXATPMATCH` / `KXWTAMATCH`, e.g. event `KXATPMATCH-26AUG17LEHFIL`
//! = "Lehecka vs Fils", with the full player name in each market's
//! `yes_sub_title`), while Polymarket carries the same matches with
//! player-name outcomes — and the two venues disagree on name form
//! ("C. Alcaraz" / "Carlos Alcaraz" / "Alcaraz").
//!
//! The canonical key is the UPPERCASE SURNAME ("ALCARAZ"), because that is
//! the only stable token across venues. Multi-word and particle surnames
//! ("De Minaur", "Davidovich Fokina", "Van De Zandschulp") are resolved via
//! an explicit alias table; everything else falls back to "last word of the
//! name", which is correct for the vast majority of tour players.
//!
//! Contributed by the Live Tennis API team (https://livetennisapi.com) —
//! vendor-affiliated; the alias table itself is plain public tour data and
//! this module performs no network calls.

use std::collections::HashMap;
use std::sync::LazyLock;

/// Irregular player-name mappings (any known form -> canonical surname key).
///
/// Only surnames that the "last word" fallback would get WRONG belong here:
/// multi-word surnames, particle surnames, and common short forms. Keys are
/// uppercase. Single-word surnames (SINNER, ALCARAZ, DJOKOVIC, ...) do not
/// need entries — the fallback already handles them.
static TENNIS_PLAYER_MAPPINGS: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| {
        let mut mappings = HashMap::new();

        // --- ATP: multi-word / particle surnames -------------------------
        mappings.insert("ALEX DE MINAUR", "DE MINAUR");
        mappings.insert("DE MINAUR", "DE MINAUR");
        mappings.insert("A. DE MINAUR", "DE MINAUR");

        mappings.insert("FELIX AUGER-ALIASSIME", "AUGER-ALIASSIME");
        mappings.insert("AUGER-ALIASSIME", "AUGER-ALIASSIME");
        mappings.insert("AUGER ALIASSIME", "AUGER-ALIASSIME");
        mappings.insert("F. AUGER-ALIASSIME", "AUGER-ALIASSIME");

        mappings.insert("ALEJANDRO DAVIDOVICH FOKINA", "DAVIDOVICH FOKINA");
        mappings.insert("DAVIDOVICH FOKINA", "DAVIDOVICH FOKINA");
        mappings.insert("A. DAVIDOVICH FOKINA", "DAVIDOVICH FOKINA");

        mappings.insert("ROBERTO BAUTISTA AGUT", "BAUTISTA AGUT");
        mappings.insert("BAUTISTA AGUT", "BAUTISTA AGUT");
        mappings.insert("R. BAUTISTA AGUT", "BAUTISTA AGUT");

        mappings.insert("PABLO CARRENO BUSTA", "CARRENO BUSTA");
        mappings.insert("CARRENO BUSTA", "CARRENO BUSTA");

        mappings.insert("BOTIC VAN DE ZANDSCHULP", "VAN DE ZANDSCHULP");
        mappings.insert("VAN DE ZANDSCHULP", "VAN DE ZANDSCHULP");
        mappings.insert("B. VAN DE ZANDSCHULP", "VAN DE ZANDSCHULP");

        mappings.insert("TALLON GRIEKSPOOR", "GRIEKSPOOR");
        mappings.insert("JAN-LENNARD STRUFF", "STRUFF");

        mappings.insert("FRANCISCO CERUNDOLO", "F. CERUNDOLO");
        mappings.insert("F. CERUNDOLO", "F. CERUNDOLO");
        mappings.insert("JUAN MANUEL CERUNDOLO", "J.M. CERUNDOLO");

        // --- WTA: multi-word / particle surnames -------------------------
        mappings.insert("BEATRIZ HADDAD MAIA", "HADDAD MAIA");
        mappings.insert("HADDAD MAIA", "HADDAD MAIA");
        mappings.insert("B. HADDAD MAIA", "HADDAD MAIA");

        mappings.insert("SARA SORRIBES TORMO", "SORRIBES TORMO");
        mappings.insert("SORRIBES TORMO", "SORRIBES TORMO");

        mappings.insert("LEYLAH FERNANDEZ", "FERNANDEZ");
        mappings.insert("ANNA KAROLINA SCHMIEDLOVA", "SCHMIEDLOVA");

        // --- Same-surname pairs need an explicit, first-initial key ------
        mappings.insert("ALEXANDER ZVEREV", "A. ZVEREV");
        mappings.insert("A. ZVEREV", "A. ZVEREV");
        mappings.insert("SASCHA ZVEREV", "A. ZVEREV");
        mappings.insert("MISCHA ZVEREV", "M. ZVEREV");
        mappings.insert("M. ZVEREV", "M. ZVEREV");

        mappings.insert("MIRRA ANDREEVA", "M. ANDREEVA");
        mappings.insert("M. ANDREEVA", "M. ANDREEVA");
        mappings.insert("ERIKA ANDREEVA", "E. ANDREEVA");
        mappings.insert("E. ANDREEVA", "E. ANDREEVA");

        mappings
    });

/// Normalize a tennis player name to its canonical surname key
/// 将网球选手姓名标准化为规范的姓氏键
///
/// Resolution order:
/// 1. the explicit alias table (irregular surnames, same-surname pairs);
/// 2. fallback: the last whitespace-separated word, uppercased — correct
///    for single-word surnames from any of "Alcaraz", "C. Alcaraz",
///    "Carlos Alcaraz".
///
/// # Examples
/// ```
/// use polytaoli::core::normalize_player_name;
///
/// assert_eq!(normalize_player_name("Carlos Alcaraz"), "ALCARAZ");
/// assert_eq!(normalize_player_name("C. Alcaraz"), "ALCARAZ");
/// assert_eq!(normalize_player_name("Alex de Minaur"), "DE MINAUR");
/// assert_eq!(normalize_player_name("Alexander Zverev"), "A. ZVEREV");
/// ```
pub fn normalize_player_name(name: &str) -> String {
    let name_upper = name.trim().to_uppercase();
    if let Some(canonical) = TENNIS_PLAYER_MAPPINGS.get(name_upper.as_str()) {
        return canonical.to_string();
    }
    name_upper
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or(&name_upper)
        .to_string()
}

/// Check whether a Kalshi 3-letter ticker code refers to this player
/// 检查 Kalshi 的三字母代码是否指向该选手
///
/// Kalshi tennis market tickers end in the first three letters of the
/// surname (`KXATPMATCH-26AUG17LEHFIL-LEH` -> Jiri Lehecka). The code is
/// lossy, so it should only ever be checked against the two players already
/// known to be in the event — never used as a global key.
///
/// # Examples
/// ```
/// use polytaoli::core::ticker_code_matches;
///
/// assert!(ticker_code_matches("LEH", "Jiri Lehecka"));
/// assert!(ticker_code_matches("FIL", "Arthur Fils"));
/// assert!(!ticker_code_matches("FIL", "Jiri Lehecka"));
/// ```
pub fn ticker_code_matches(code: &str, player_name: &str) -> bool {
    let surname = normalize_player_name(player_name);
    let surname_letters: String = surname
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .collect();
    surname_letters
        .to_uppercase()
        .starts_with(&code.trim().to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_names() {
        assert_eq!(normalize_player_name("Carlos Alcaraz"), "ALCARAZ");
        assert_eq!(normalize_player_name("Jannik Sinner"), "SINNER");
        assert_eq!(normalize_player_name("Novak Djokovic"), "DJOKOVIC");
        assert_eq!(normalize_player_name("Aryna Sabalenka"), "SABALENKA");
    }

    #[test]
    fn test_initial_forms() {
        assert_eq!(normalize_player_name("C. Alcaraz"), "ALCARAZ");
        assert_eq!(normalize_player_name("J. Sinner"), "SINNER");
        assert_eq!(normalize_player_name("N. Djokovic"), "DJOKOVIC");
    }

    #[test]
    fn test_surname_only() {
        assert_eq!(normalize_player_name("Alcaraz"), "ALCARAZ");
        assert_eq!(normalize_player_name("alcaraz"), "ALCARAZ");
    }

    #[test]
    fn test_multi_word_surnames() {
        assert_eq!(normalize_player_name("Alex de Minaur"), "DE MINAUR");
        assert_eq!(normalize_player_name("De Minaur"), "DE MINAUR");
        assert_eq!(
            normalize_player_name("Alejandro Davidovich Fokina"),
            "DAVIDOVICH FOKINA"
        );
        assert_eq!(
            normalize_player_name("Botic van de Zandschulp"),
            "VAN DE ZANDSCHULP"
        );
        assert_eq!(normalize_player_name("Beatriz Haddad Maia"), "HADDAD MAIA");
    }

    #[test]
    fn test_hyphenated_surnames() {
        assert_eq!(
            normalize_player_name("Felix Auger-Aliassime"),
            "AUGER-ALIASSIME"
        );
        assert_eq!(normalize_player_name("Auger-Aliassime"), "AUGER-ALIASSIME");
    }

    #[test]
    fn test_same_surname_pairs_stay_distinct() {
        assert_eq!(normalize_player_name("Alexander Zverev"), "A. ZVEREV");
        assert_eq!(normalize_player_name("Mischa Zverev"), "M. ZVEREV");
        assert_ne!(
            normalize_player_name("Alexander Zverev"),
            normalize_player_name("Mischa Zverev")
        );
        assert_ne!(
            normalize_player_name("Mirra Andreeva"),
            normalize_player_name("Erika Andreeva")
        );
    }

    #[test]
    fn test_cross_venue_agreement() {
        // The property the matcher needs: every venue's form of the same
        // player collapses to one key.
        for (a, b) in [
            ("Carlos Alcaraz", "C. Alcaraz"),
            ("Carlos Alcaraz", "Alcaraz"),
            ("Alex de Minaur", "de Minaur"),
            ("Felix Auger-Aliassime", "Auger-Aliassime"),
        ] {
            assert_eq!(normalize_player_name(a), normalize_player_name(b));
        }
    }

    #[test]
    fn test_ticker_codes() {
        // Real codes observed on KXATPMATCH-26AUG17LEHFIL
        assert!(ticker_code_matches("LEH", "Jiri Lehecka"));
        assert!(ticker_code_matches("FIL", "Arthur Fils"));
        assert!(!ticker_code_matches("FIL", "Jiri Lehecka"));
        // Case-insensitive, and multi-word surnames use letters only
        assert!(ticker_code_matches("dem", "Alex de Minaur"));
    }

    #[test]
    fn test_unknown_player_falls_back_to_last_word() {
        assert_eq!(normalize_player_name("Future Newcomer"), "NEWCOMER");
    }
}

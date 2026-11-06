//! NBA team name normalization
//!
//! Provides standardized team abbreviations for consistent matching
//! between different platforms (Kalshi, Polymarket).

use std::collections::HashMap;
use std::sync::LazyLock;

/// NBA team mappings (full name/alias -> standard abbreviation)
static NBA_TEAM_MAPPINGS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut mappings = HashMap::new();

    // Eastern Conference
    mappings.insert("ATLANTA HAWKS", "ATL");
    mappings.insert("HAWKS", "ATL");
    mappings.insert("ATL", "ATL");

    mappings.insert("BOSTON CELTICS", "BOS");
    mappings.insert("CELTICS", "BOS");
    mappings.insert("BOS", "BOS");

    mappings.insert("BROOKLYN NETS", "BKN");
    mappings.insert("NETS", "BKN");
    mappings.insert("BKN", "BKN");
    mappings.insert("BRK", "BKN");

    mappings.insert("CHARLOTTE HORNETS", "CHA");
    mappings.insert("HORNETS", "CHA");
    mappings.insert("CHA", "CHA");
    mappings.insert("CHO", "CHA");

    mappings.insert("CHICAGO BULLS", "CHI");
    mappings.insert("BULLS", "CHI");
    mappings.insert("CHI", "CHI");

    mappings.insert("CLEVELAND CAVALIERS", "CLE");
    mappings.insert("CAVALIERS", "CLE");
    mappings.insert("CAVS", "CLE");
    mappings.insert("CLE", "CLE");

    mappings.insert("DETROIT PISTONS", "DET");
    mappings.insert("PISTONS", "DET");
    mappings.insert("DET", "DET");

    mappings.insert("INDIANA PACERS", "IND");
    mappings.insert("PACERS", "IND");
    mappings.insert("IND", "IND");

    mappings.insert("MIAMI HEAT", "MIA");
    mappings.insert("HEAT", "MIA");
    mappings.insert("MIA", "MIA");

    mappings.insert("MILWAUKEE BUCKS", "MIL");
    mappings.insert("BUCKS", "MIL");
    mappings.insert("MIL", "MIL");

    mappings.insert("NEW YORK KNICKS", "NYK");
    mappings.insert("KNICKS", "NYK");
    mappings.insert("NYK", "NYK");
    mappings.insert("NY", "NYK");

    mappings.insert("ORLANDO MAGIC", "ORL");
    mappings.insert("MAGIC", "ORL");
    mappings.insert("ORL", "ORL");

    mappings.insert("PHILADELPHIA 76ERS", "PHI");
    mappings.insert("76ERS", "PHI");
    mappings.insert("SIXERS", "PHI");
    mappings.insert("PHI", "PHI");

    mappings.insert("TORONTO RAPTORS", "TOR");
    mappings.insert("RAPTORS", "TOR");
    mappings.insert("TOR", "TOR");

    mappings.insert("WASHINGTON WIZARDS", "WAS");
    mappings.insert("WIZARDS", "WAS");
    mappings.insert("WAS", "WAS");
    mappings.insert("WSH", "WAS");

    // Western Conference
    mappings.insert("DALLAS MAVERICKS", "DAL");
    mappings.insert("MAVERICKS", "DAL");
    mappings.insert("MAVS", "DAL");
    mappings.insert("DAL", "DAL");

    mappings.insert("DENVER NUGGETS", "DEN");
    mappings.insert("NUGGETS", "DEN");
    mappings.insert("DEN", "DEN");

    mappings.insert("GOLDEN STATE WARRIORS", "GSW");
    mappings.insert("WARRIORS", "GSW");
    mappings.insert("GSW", "GSW");
    mappings.insert("GS", "GSW");

    mappings.insert("HOUSTON ROCKETS", "HOU");
    mappings.insert("ROCKETS", "HOU");
    mappings.insert("HOU", "HOU");

    mappings.insert("LOS ANGELES CLIPPERS", "LAC");
    mappings.insert("CLIPPERS", "LAC");
    mappings.insert("LAC", "LAC");
    mappings.insert("LA CLIPPERS", "LAC");

    mappings.insert("LOS ANGELES LAKERS", "LAL");
    mappings.insert("LAKERS", "LAL");
    mappings.insert("LAL", "LAL");
    mappings.insert("LA LAKERS", "LAL");

    mappings.insert("MEMPHIS GRIZZLIES", "MEM");
    mappings.insert("GRIZZLIES", "MEM");
    mappings.insert("MEM", "MEM");

    mappings.insert("MINNESOTA TIMBERWOLVES", "MIN");
    mappings.insert("TIMBERWOLVES", "MIN");
    mappings.insert("WOLVES", "MIN");
    mappings.insert("MIN", "MIN");

    mappings.insert("NEW ORLEANS PELICANS", "NOP");
    mappings.insert("PELICANS", "NOP");
    mappings.insert("NOP", "NOP");
    mappings.insert("NO", "NOP");

    mappings.insert("OKLAHOMA CITY THUNDER", "OKC");
    mappings.insert("THUNDER", "OKC");
    mappings.insert("OKC", "OKC");

    mappings.insert("PHOENIX SUNS", "PHX");
    mappings.insert("SUNS", "PHX");
    mappings.insert("PHX", "PHX");
    mappings.insert("PHO", "PHX");

    mappings.insert("PORTLAND TRAIL BLAZERS", "POR");
    mappings.insert("TRAIL BLAZERS", "POR");
    mappings.insert("BLAZERS", "POR");
    mappings.insert("POR", "POR");

    mappings.insert("SACRAMENTO KINGS", "SAC");
    mappings.insert("KINGS", "SAC");
    mappings.insert("SAC", "SAC");

    mappings.insert("SAN ANTONIO SPURS", "SAS");
    mappings.insert("SPURS", "SAS");
    mappings.insert("SAS", "SAS");

    mappings.insert("UTAH JAZZ", "UTA");
    mappings.insert("JAZZ", "UTA");
    mappings.insert("UTA", "UTA");

    mappings
});

/// Normalize team name to standard 3-letter abbreviation
///
/// # Arguments
/// * `name` - Team name in any format (full name, nickname, abbreviation)
///
/// # Returns
/// Standard 3-letter abbreviation (e.g., "LAL", "BOS", "GSW")
///
/// # Examples
/// ```
/// use polytaoli::core::normalize_team_name;
///
/// assert_eq!(normalize_team_name("Lakers"), "LAL");
/// assert_eq!(normalize_team_name("Los Angeles Lakers"), "LAL");
/// assert_eq!(normalize_team_name("LAL"), "LAL");
/// ```
pub fn normalize_team_name(name: &str) -> String {
    let name_upper = name.trim().to_uppercase();
    NBA_TEAM_MAPPINGS
        .get(name_upper.as_str())
        .map(|s| s.to_string())
        .unwrap_or(name_upper)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_full_names() {
        assert_eq!(normalize_team_name("Los Angeles Lakers"), "LAL");
        assert_eq!(normalize_team_name("Boston Celtics"), "BOS");
        assert_eq!(normalize_team_name("Golden State Warriors"), "GSW");
    }

    #[test]
    fn test_normalize_nicknames() {
        assert_eq!(normalize_team_name("Lakers"), "LAL");
        assert_eq!(normalize_team_name("Celtics"), "BOS");
        assert_eq!(normalize_team_name("Warriors"), "GSW");
    }

    #[test]
    fn test_normalize_abbreviations() {
        assert_eq!(normalize_team_name("LAL"), "LAL");
        assert_eq!(normalize_team_name("BOS"), "BOS");
        assert_eq!(normalize_team_name("GSW"), "GSW");
    }

    #[test]
    fn test_normalize_case_insensitive() {
        assert_eq!(normalize_team_name("lakers"), "LAL");
        assert_eq!(normalize_team_name("LAKERS"), "LAL");
        assert_eq!(normalize_team_name("LaKeRs"), "LAL");
    }

    #[test]
    fn test_normalize_alternate_abbrevs() {
        assert_eq!(normalize_team_name("BRK"), "BKN");
        assert_eq!(normalize_team_name("CHO"), "CHA");
        assert_eq!(normalize_team_name("WSH"), "WAS");
        assert_eq!(normalize_team_name("PHO"), "PHX");
    }

    #[test]
    fn test_unknown_team() {
        // Unknown team names should return uppercase version
        assert_eq!(normalize_team_name("Unknown Team"), "UNKNOWN TEAM");
    }
}

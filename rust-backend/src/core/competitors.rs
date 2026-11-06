//! Conservative competitor identity normalization for tennis match winners.
//!
//! Unlike NBA teams, tennis competitors do not have a stable abbreviation
//! registry. This module only normalizes presentation differences and never
//! expands initials or infers a player from a surname.

/// Normalize a tennis player or pair name for exact cross-exchange matching.
///
/// Case, punctuation, whitespace, and pair connectors (`&`, `/`, `and`) are
/// normalized. Names without at least two meaningful words, placeholders, and
/// malformed pair connectors are rejected so ambiguous markets cannot match.
pub fn normalize_competitor_name(name: &str) -> Option<String> {
    let mut normalized = String::with_capacity(name.len());

    for character in name.trim().chars() {
        if character.is_alphanumeric() {
            normalized.extend(character.to_uppercase());
        } else if matches!(character, '&' | '/' | '+') {
            normalized.push_str(" AND ");
        } else {
            normalized.push(' ');
        }
    }

    let words: Vec<&str> = normalized.split_whitespace().collect();
    let meaningful_words: Vec<&str> = words
        .iter()
        .copied()
        .filter(|word| *word != "AND")
        .collect();

    if meaningful_words.len() < 2
        || meaningful_words.iter().any(|word| word.chars().count() < 2)
        || words.first() == Some(&"AND")
        || words.last() == Some(&"AND")
        || words.windows(2).any(|pair| pair == ["AND", "AND"])
    {
        return None;
    }

    let canonical = words.join(" ");
    if matches!(
        canonical.as_str(),
        "TBD" | "TO BE DETERMINED" | "PLAYER A" | "PLAYER B" | "TEAM A" | "TEAM B"
    ) {
        return None;
    }

    Some(canonical)
}

/// Build a stable, order-independent event key from two canonical competitors.
pub fn competitor_event_name(first: &str, second: &str) -> Option<String> {
    if first == second {
        return None;
    }

    let (first, second) = if first < second {
        (first, second)
    } else {
        (second, first)
    };
    Some(format!("{first} VS {second}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_punctuation_whitespace_and_pair_connectors() {
        assert_eq!(
            normalize_competitor_name("  Juan-Manuel  Cerundolo "),
            Some("JUAN MANUEL CERUNDOLO".to_string())
        );
        assert_eq!(
            normalize_competitor_name("Alice Smith / Beth Jones"),
            normalize_competitor_name("Alice Smith & Beth Jones")
        );
    }

    #[test]
    fn rejects_ambiguous_competitors() {
        assert_eq!(normalize_competitor_name("Cerundolo"), None);
        assert_eq!(normalize_competitor_name("J. Cerundolo"), None);
        assert_eq!(normalize_competitor_name("TBD"), None);
        assert_eq!(normalize_competitor_name("Player A"), None);
    }

    #[test]
    fn builds_order_independent_event_names() {
        assert_eq!(
            competitor_event_name("AUGER ALIASSIME", "JUAN MANUEL CERUNDOLO"),
            Some("AUGER ALIASSIME VS JUAN MANUEL CERUNDOLO".to_string())
        );
    }
}

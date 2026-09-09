//! System-level attributes shared by the journal and EDDN paths.

/// The security level a system shows on the galaxy map, from whatever
/// form a source carried it in.
///
/// The journal writes `SystemSecurity` as a localisation symbol
/// (`$SYSTEM_SECURITY_low;`, `$GAlAXY_MAP_INFO_state_anarchy;`) beside a
/// `SystemSecurity_Localised` string; EDDN strips the `_Localised` fields,
/// so its journal frames carry only the symbol. Spansh dumps carry the
/// display names (`Low`, `Medium`, `High`, `Anarchy`). Every store keeps
/// the display name, so the three sources compare and render alike.
///
/// A symbol this does not know keeps its last `_` segment, capitalised:
/// `$SYSTEM_SECURITY_whatever;` renders as `Whatever` rather than as the
/// raw key.
pub fn security_name(raw: &str) -> String {
    let trimmed = raw.trim();
    let Some(symbol) = trimmed.strip_prefix('$') else {
        return trimmed.to_string();
    };
    let symbol = symbol.strip_suffix(';').unwrap_or(symbol);
    let key = symbol.rsplit('_').next().unwrap_or(symbol);
    match key.to_ascii_lowercase().as_str() {
        "low" => "Low".to_string(),
        "medium" => "Medium".to_string(),
        "high" => "High".to_string(),
        "anarchy" => "Anarchy".to_string(),
        "lawless" => "Lawless".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => trimmed.to_string(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_security_symbols_become_display_names() {
        assert_eq!(security_name("$SYSTEM_SECURITY_low;"), "Low");
        assert_eq!(security_name("$SYSTEM_SECURITY_medium;"), "Medium");
        assert_eq!(security_name("$SYSTEM_SECURITY_high;"), "High");
        // The odd capitalisation is the game's own.
        assert_eq!(security_name("$GAlAXY_MAP_INFO_state_anarchy;"), "Anarchy");
        assert_eq!(security_name("$GAlAXY_MAP_INFO_state_lawless;"), "Lawless");
    }

    #[test]
    fn display_names_pass_through_untouched() {
        for name in ["Low", "Medium", "High", "Anarchy", ""] {
            assert_eq!(security_name(name), name);
        }
    }

    #[test]
    fn an_unknown_symbol_is_readable_not_raw() {
        assert_eq!(security_name("$SYSTEM_SECURITY_martial;"), "Martial");
        assert_eq!(security_name("$SYSTEM_SECURITY_;"), "$SYSTEM_SECURITY_;");
    }
}

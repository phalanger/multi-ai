//! Scrape rule files (TOML) and the built-in default rules.

use mai_protocol::ScrapeRules;

const DEFAULT_RULES: &str = include_str!("../rules/default.toml");

/// Parse a rules file in the `rules/default.toml` format.
pub fn parse_rules(text: &str) -> Result<ScrapeRules, toml::de::Error> {
    toml::from_str(text)
}

/// Built-in rules shipped with the probe.
pub fn default_rules() -> ScrapeRules {
    parse_rules(DEFAULT_RULES).expect("built-in rules/default.toml is valid")
}

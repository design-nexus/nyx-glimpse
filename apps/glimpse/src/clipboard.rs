//! Reading a `cb …` query (Nyx Clipboard). A query whose first word is `cb` or `clip`
//! belongs to the clipboard history alone: no other provider, plugin or semantic search
//! sees it, and it is never learned.

/// A `cb` query's search text (prefixes such as `img:` pass through), or `None`.
pub fn parse(text: &str) -> Option<String> {
    let text = text.trim_start();
    let (first, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    (first.eq_ignore_ascii_case("cb") || first.eq_ignore_ascii_case("clip")).then(|| rest.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes() {
        assert_eq!(parse("cb"), Some(String::new()));
        assert_eq!(parse("  CB  github "), Some("github".into()));
        assert_eq!(parse("clip img: cat"), Some("img: cat".into()));
        assert_eq!(parse("cbx"), None);
        assert_eq!(parse("clipboard"), None);
        assert_eq!(parse("github cb"), None);
    }
}

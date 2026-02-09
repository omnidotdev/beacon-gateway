//! URL detection in text

use regex::Regex;
use std::sync::LazyLock;

/// Regex for detecting URLs
static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://[^\s<>\[\](){}]+").expect("valid regex")
});

/// Detect all URLs in a string
#[must_use]
pub fn detect_urls(text: &str) -> Vec<String> {
    URL_REGEX
        .find_iter(text)
        .map(|m| {
            let url = m.as_str();
            // Clean trailing punctuation
            url.trim_end_matches(|c| matches!(c, '.' | ',' | '!' | '?' | ')' | ']' | '}'))
                .to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_simple_url() {
        let urls = detect_urls("Check out https://example.com for more info");
        assert_eq!(urls, vec!["https://example.com"]);
    }

    #[test]
    fn test_detect_multiple_urls() {
        let urls = detect_urls("See https://foo.com and https://bar.com");
        assert_eq!(urls, vec!["https://foo.com", "https://bar.com"]);
    }

    #[test]
    fn test_detect_url_with_path() {
        let urls = detect_urls("Visit https://example.com/path/to/page?query=1");
        assert_eq!(urls, vec!["https://example.com/path/to/page?query=1"]);
    }

    #[test]
    fn test_strip_trailing_punctuation() {
        let urls = detect_urls("Check https://example.com.");
        assert_eq!(urls, vec!["https://example.com"]);

        let urls = detect_urls("See https://example.com, it's great!");
        assert_eq!(urls, vec!["https://example.com"]);
    }

    #[test]
    fn test_no_urls() {
        let urls = detect_urls("This has no URLs in it");
        assert!(urls.is_empty());
    }

    #[test]
    fn test_http_url() {
        let urls = detect_urls("Old site: http://example.com");
        assert_eq!(urls, vec!["http://example.com"]);
    }
}

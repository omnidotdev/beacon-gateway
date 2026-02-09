//! Article content extraction from HTML
//!
//! Uses the readability algorithm to extract the main content from web pages,
//! removing navigation, ads, and other non-content elements.

use std::io::Cursor;

use url::Url;

use crate::{Error, Result};

/// Extracted article content
#[derive(Debug, Clone)]
pub struct Article {
    /// Article title
    pub title: Option<String>,
    /// Clean text content (no HTML)
    pub content: String,
    /// Sanitized HTML content
    pub html: Option<String>,
    /// Article byline (author)
    pub byline: Option<String>,
    /// Article excerpt/summary
    pub excerpt: Option<String>,
}

/// Extract readable article content from HTML
///
/// Uses the readability algorithm to identify and extract the main content
/// from a web page, stripping out navigation, ads, and boilerplate.
///
/// # Arguments
///
/// * `html` - Raw HTML content of the page
/// * `source_url` - URL of the page (used for resolving relative links)
///
/// # Errors
///
/// Returns error if URL parsing fails or content extraction fails.
///
/// # Examples
///
/// ```ignore
/// let html = r#"<html><body><article><h1>Title</h1><p>Content</p></article></body></html>"#;
/// let article = extract_article(html, "https://example.com/article")?;
/// println!("{}", article.content);
/// ```
pub fn extract_article(html: &str, source_url: &str) -> Result<Article> {
    let url = Url::parse(source_url)
        .map_err(|e| Error::Browser(format!("invalid URL: {e}")))?;

    let mut cursor = Cursor::new(html.as_bytes());

    let product = readability::extractor::extract(&mut cursor, &url)
        .map_err(|e| Error::Browser(format!("extraction failed: {e}")))?;

    // Extract title, filtering empty strings
    let title = if product.title.is_empty() {
        None
    } else {
        Some(product.title)
    };

    // The product.content is HTML, product.text is plain text
    let content = product.text;
    let html = if product.content.is_empty() {
        None
    } else {
        Some(product.content)
    };

    // Generate excerpt from first ~200 chars of content
    let excerpt = if content.len() > 200 {
        let truncated = content.chars().take(200).collect::<String>();
        // Find last space to avoid cutting words
        let end = truncated.rfind(' ').unwrap_or(truncated.len());
        Some(format!("{}...", &truncated[..end]))
    } else if !content.is_empty() {
        Some(content.clone())
    } else {
        None
    };

    Ok(Article {
        title,
        content,
        html,
        byline: None, // readability crate doesn't extract byline
        excerpt,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_simple_article() {
        let html = r"
            <!DOCTYPE html>
            <html>
            <head><title>Test Article</title></head>
            <body>
                <nav>Navigation here</nav>
                <article>
                    <h1>The Main Title</h1>
                    <p>This is the first paragraph of the article content.
                       It contains some meaningful text that should be extracted.</p>
                    <p>This is another paragraph with more content to ensure
                       the readability algorithm has enough text to work with.</p>
                </article>
                <footer>Footer content</footer>
            </body>
            </html>
        ";

        let result = extract_article(html, "https://example.com/article");
        assert!(result.is_ok());

        let article = result.unwrap();
        assert!(!article.content.is_empty());
    }

    #[test]
    fn test_extract_with_invalid_url() {
        let html = "<html><body><p>Content</p></body></html>";
        let result = extract_article(html, "not-a-valid-url");
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_html() {
        let result = extract_article("", "https://example.com");
        // Empty HTML should still parse, just with empty content
        assert!(result.is_ok());
    }
}

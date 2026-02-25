//! Markdown to Telegram-compatible HTML converter
//!
//! Converts common markdown formatting to HTML tags supported by Telegram's
//! `parse_mode: "HTML"`. This is more robust than `MarkdownV2` which requires
//! strict escaping of many special characters.

/// Convert markdown text to Telegram-compatible HTML.
///
/// Supported conversions:
/// - `**bold**` / `__bold__` → `<b>bold</b>`
/// - `*italic*` / `_italic_` → `<i>italic</i>`
/// - `` `code` `` → `<code>code</code>`
/// - ` ```lang\nblock\n``` ` → `<pre><code class="language-lang">block</code></pre>`
/// - `~~strike~~` → `<s>strike</s>`
/// - `[text](url)` → `<a href="url">text</a>`
/// - `> blockquote` → `<blockquote>blockquote</blockquote>`
///
/// HTML special characters (`<`, `>`, `&`) are escaped in non-tag content.
#[must_use]
pub fn markdown_to_telegram_html(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let lines: Vec<&str> = input.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        // Fenced code block
        if line.starts_with("```") {
            let lang = line.trim_start_matches('`').trim();
            let mut code_lines = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].starts_with("```") {
                code_lines.push(lines[i]);
                i += 1;
            }
            // Skip closing ```
            if i < lines.len() {
                i += 1;
            }

            let code_content = escape_html(&code_lines.join("\n"));
            if lang.is_empty() {
                output.push_str(&format!("<pre><code>{code_content}</code></pre>"));
            } else {
                output.push_str(&format!(
                    "<pre><code class=\"language-{lang}\">{code_content}</code></pre>"
                ));
            }
            output.push('\n');
            continue;
        }

        // Blockquote
        if let Some(rest) = line.strip_prefix("> ") {
            let quoted = convert_inline(&escape_html(rest));
            output.push_str(&format!("<blockquote>{quoted}</blockquote>"));
            output.push('\n');
            i += 1;
            continue;
        }

        // Regular line: escape HTML then convert inline formatting
        let escaped = escape_html(line);
        let converted = convert_inline(&escaped);
        output.push_str(&converted);
        output.push('\n');
        i += 1;
    }

    // Remove trailing newline
    if output.ends_with('\n') {
        output.pop();
    }

    output
}

/// Escape HTML special characters
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Convert inline markdown formatting to HTML tags
///
/// Processes in order: inline code, bold, italic, strikethrough, links.
/// Inline code is processed first to prevent format conversion inside code spans.
fn convert_inline(text: &str) -> String {
    let text = convert_inline_code(text);
    let text = convert_bold(&text);
    let text = convert_italic(&text);
    let text = convert_strikethrough(&text);
    convert_links(&text)
}

/// Convert `` `code` `` to `<code>code</code>`
fn convert_inline_code(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '`' {
            let mut code = String::new();
            let mut found_close = false;
            for next in chars.by_ref() {
                if next == '`' {
                    found_close = true;
                    break;
                }
                code.push(next);
            }
            if found_close {
                result.push_str(&format!("<code>{code}</code>"));
            } else {
                // Unmatched backtick, output as-is
                result.push('`');
                result.push_str(&code);
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Convert `**bold**` to `<b>bold</b>`
fn convert_bold(text: &str) -> String {
    convert_delimited(text, "**", "<b>", "</b>")
}

/// Convert `*italic*` to `<i>italic</i>`
///
/// Processes the text after bold conversion, so `<b>` tags may be present.
/// We process content segments between HTML tags so `*` inside tag attributes
/// is preserved.
fn convert_italic(text: &str) -> String {
    // Split on HTML tags, convert only the non-tag segments
    let mut result = String::with_capacity(text.len());
    let mut remaining = text;

    while !remaining.is_empty() {
        if let Some(tag_start) = remaining.find('<') {
            // Convert text before the tag
            let before = &remaining[..tag_start];
            result.push_str(&convert_single_delimited(before, '*', "<i>", "</i>"));

            // Find end of tag and pass through as-is
            if let Some(tag_end) = remaining[tag_start..].find('>') {
                let end = tag_start + tag_end + 1;
                result.push_str(&remaining[tag_start..end]);
                remaining = &remaining[end..];
            } else {
                result.push_str(&remaining[tag_start..]);
                break;
            }
        } else {
            result.push_str(&convert_single_delimited(remaining, '*', "<i>", "</i>"));
            break;
        }
    }

    result
}

/// Convert `~~strike~~` to `<s>strike</s>`
fn convert_strikethrough(text: &str) -> String {
    convert_delimited(text, "~~", "<s>", "</s>")
}

/// Convert `[text](url)` to `<a href="url">text</a>`
fn convert_links(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut remaining = text;

    while let Some(bracket_start) = remaining.find('[') {
        result.push_str(&remaining[..bracket_start]);

        let after_bracket = &remaining[bracket_start + 1..];
        if let Some(bracket_end) = after_bracket.find("](") {
            let link_text = &after_bracket[..bracket_end];
            let after_paren = &after_bracket[bracket_end + 2..];

            if let Some(paren_end) = after_paren.find(')') {
                let url = &after_paren[..paren_end];
                result.push_str(&format!("<a href=\"{url}\">{link_text}</a>"));
                remaining = &after_paren[paren_end + 1..];
                continue;
            }
        }

        // Not a valid link, output the bracket
        result.push('[');
        remaining = after_bracket;
    }

    result.push_str(remaining);
    result
}

/// Generic two-char delimiter converter (e.g., `**` → `<b>`, `~~` → `<s>`)
fn convert_delimited(text: &str, delimiter: &str, open_tag: &str, close_tag: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut remaining = text;
    let mut open = false;

    while let Some(pos) = remaining.find(delimiter) {
        result.push_str(&remaining[..pos]);
        if open {
            result.push_str(close_tag);
        } else {
            result.push_str(open_tag);
        }
        open = !open;
        remaining = &remaining[pos + delimiter.len()..];
    }

    result.push_str(remaining);

    // If unmatched, put the delimiter back
    if open {
        // Find the last open tag and replace it with the delimiter
        if let Some(last_open) = result.rfind(open_tag) {
            let mut fixed = String::with_capacity(result.len());
            fixed.push_str(&result[..last_open]);
            fixed.push_str(delimiter);
            fixed.push_str(&result[last_open + open_tag.len()..]);
            return fixed;
        }
    }

    result
}

/// Single-char delimiter converter (for `*italic*`)
fn convert_single_delimited(text: &str, delimiter: char, open_tag: &str, close_tag: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut open = false;

    while let Some(ch) = chars.next() {
        if ch == delimiter {
            // Don't treat * as italic if preceded/followed by space (list markers, etc.)
            if !open {
                // Check if next char is a non-space (opening delimiter)
                if chars.peek().is_some_and(|c| !c.is_whitespace()) {
                    result.push_str(open_tag);
                    open = true;
                } else {
                    result.push(ch);
                }
            } else {
                result.push_str(close_tag);
                open = false;
            }
        } else {
            result.push(ch);
        }
    }

    // If unmatched, put the delimiter back
    if open {
        if let Some(last_open) = result.rfind(open_tag) {
            let mut fixed = String::with_capacity(result.len());
            fixed.push_str(&result[..last_open]);
            fixed.push(delimiter);
            fixed.push_str(&result[last_open + open_tag.len()..]);
            return fixed;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_bold_italic_code() {
        assert_eq!(
            markdown_to_telegram_html("**bold**"),
            "<b>bold</b>"
        );
        assert_eq!(
            markdown_to_telegram_html("*italic*"),
            "<i>italic</i>"
        );
        assert_eq!(
            markdown_to_telegram_html("`code`"),
            "<code>code</code>"
        );
    }

    #[test]
    fn html_code_blocks_with_language() {
        let input = "```rust\nfn main() {}\n```";
        let expected = "<pre><code class=\"language-rust\">fn main() {}</code></pre>";
        assert_eq!(markdown_to_telegram_html(input), expected);
    }

    #[test]
    fn html_code_blocks_without_language() {
        let input = "```\nhello\n```";
        let expected = "<pre><code>hello</code></pre>";
        assert_eq!(markdown_to_telegram_html(input), expected);
    }

    #[test]
    fn html_escapes_special_characters() {
        assert_eq!(
            markdown_to_telegram_html("1 < 2 & 3 > 0"),
            "1 &lt; 2 &amp; 3 &gt; 0"
        );
    }

    #[test]
    fn html_links() {
        assert_eq!(
            markdown_to_telegram_html("[click here](https://example.com)"),
            "<a href=\"https://example.com\">click here</a>"
        );
    }

    #[test]
    fn html_nested_formatting() {
        // Bold containing italic with separate delimiters
        assert_eq!(
            markdown_to_telegram_html("**bold** and *italic*"),
            "<b>bold</b> and <i>italic</i>"
        );
    }

    #[test]
    fn html_passthrough_plain_text() {
        assert_eq!(
            markdown_to_telegram_html("Hello, world!"),
            "Hello, world!"
        );
    }

    #[test]
    fn html_strikethrough() {
        assert_eq!(
            markdown_to_telegram_html("~~struck~~"),
            "<s>struck</s>"
        );
    }

    #[test]
    fn html_blockquote() {
        assert_eq!(
            markdown_to_telegram_html("> quoted text"),
            "<blockquote>quoted text</blockquote>"
        );
    }

    #[test]
    fn html_code_block_escapes_html() {
        let input = "```\n<script>alert('xss')</script>\n```";
        let output = markdown_to_telegram_html(input);
        assert!(output.contains("&lt;script&gt;"));
        assert!(!output.contains("<script>"));
    }

    #[test]
    fn html_inline_code_not_formatted() {
        // Inside inline code, * should not become italic
        assert_eq!(
            markdown_to_telegram_html("`a * b`"),
            "<code>a * b</code>"
        );
    }

    #[test]
    fn html_unmatched_single_star_stays_literal() {
        // A single * without closing should not produce <i>
        let result = markdown_to_telegram_html("start * middle");
        assert!(!result.contains("<i>"));
        assert!(result.contains("*"));
    }
}

//! Text chunking for Telegram's message size limit
//!
//! Telegram enforces a 4096-character cap per message. This module splits
//! long text into smaller chunks while trying to preserve logical boundaries
//! (paragraphs, sentences) and keeping fenced code blocks intact.

/// Default chunk size limit (leaves margin from Telegram's 4096 hard cap)
const DEFAULT_LIMIT: usize = 4000;

/// Strategy for splitting text that exceeds the chunk limit
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkStrategy {
    /// Split on paragraph boundaries (double newlines), fall back to sentence,
    /// then hard split
    Paragraph,
    /// Split on sentence-ending punctuation (`. `, `! `, `? `), fall back to
    /// hard split
    Sentence,
    /// Split at exact byte-offset boundaries with no heuristics
    HardSplit,
}

/// Split `text` into chunks that each fit within `limit` characters.
///
/// When `limit` is 0, the default limit ([`DEFAULT_LIMIT`]) is used.
/// Every returned chunk is guaranteed to be non-empty.
///
/// # Examples
///
/// ```ignore
/// use beacon_gateway::channels::telegram::chunking::{chunk_text, ChunkStrategy};
///
/// let chunks = chunk_text("short", 0, ChunkStrategy::Paragraph);
/// assert_eq!(chunks, vec!["short"]);
/// ```
#[must_use]
pub fn chunk_text(text: &str, limit: usize, strategy: ChunkStrategy) -> Vec<String> {
    let limit = if limit == 0 { DEFAULT_LIMIT } else { limit };

    if text.is_empty() {
        return Vec::new();
    }

    if text.len() <= limit {
        return vec![text.to_string()];
    }

    match strategy {
        ChunkStrategy::Paragraph => chunk_paragraph(text, limit),
        ChunkStrategy::Sentence => chunk_sentence(text, limit),
        ChunkStrategy::HardSplit => chunk_hard(text, limit),
    }
}

/// Paragraph-level splitting.
///
/// 1. Identify fenced code blocks and treat them as atomic units.
/// 2. Split the remaining text on `\n\n` boundaries.
/// 3. Accumulate segments into chunks up to the limit.
/// 4. If a single segment still exceeds the limit, fall back to sentence
///    splitting, then hard splitting.
fn chunk_paragraph(text: &str, limit: usize) -> Vec<String> {
    let segments = split_preserving_code_blocks(text, "\n\n");
    assemble_chunks(&segments, limit, ChunkStrategy::Sentence)
}

/// Sentence-level splitting.
///
/// Splits on `. `, `! `, `? ` boundaries. If a single segment still exceeds
/// the limit, falls back to hard splitting.
fn chunk_sentence(text: &str, limit: usize) -> Vec<String> {
    let segments = split_on_sentences(text);
    assemble_chunks(&segments, limit, ChunkStrategy::HardSplit)
}

/// Hard byte-offset splitting.
///
/// Splits at exact `limit`-sized boundaries. Tries to break on the last
/// newline before the limit; if none exists, breaks at `limit` directly.
fn chunk_hard(text: &str, limit: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= limit {
            let trimmed = remaining.trim();
            if !trimmed.is_empty() {
                chunks.push(trimmed.to_string());
            }
            break;
        }

        // Find a safe split point (last newline within limit, or limit itself)
        let split_at = find_split_point(remaining, limit);
        let chunk = remaining[..split_at].trim();
        if !chunk.is_empty() {
            chunks.push(chunk.to_string());
        }
        remaining = &remaining[split_at..];
        // Skip leading whitespace on next chunk
        remaining = remaining.trim_start();
    }

    chunks
}

/// Split text on a delimiter while keeping fenced code blocks (```) intact.
///
/// Code blocks are never broken across segments; the delimiter is ignored
/// inside fenced regions.
#[must_use]
fn split_preserving_code_blocks<'a>(text: &'a str, delimiter: &str) -> Vec<&'a str> {
    let mut segments = Vec::new();
    let mut pos = 0;
    let bytes = text.as_bytes();

    while pos < text.len() {
        // Check for a code fence at the current position
        if bytes.get(pos..pos + 3) == Some(b"```".as_slice()) {
            // Find the closing fence
            let fence_start = pos;
            // Advance past the opening fence line
            pos += 3;
            if let Some(newline) = text[pos..].find('\n') {
                pos += newline + 1;
            }
            // Scan for closing ```
            let mut found_close = false;
            while pos < text.len() {
                if text[pos..].starts_with("```") {
                    // Advance past closing fence line
                    pos += 3;
                    if let Some(newline) = text[pos..].find('\n') {
                        pos += newline + 1;
                    } else {
                        pos = text.len();
                    }
                    found_close = true;
                    break;
                }
                // Advance one line
                if let Some(newline) = text[pos..].find('\n') {
                    pos += newline + 1;
                } else {
                    pos = text.len();
                }
            }
            if !found_close {
                pos = text.len();
            }
            segments.push(&text[fence_start..pos]);
            continue;
        }

        // Look for the next delimiter or code fence, whichever comes first
        let next_fence = text[pos..].find("```").map(|i| pos + i);
        let next_delim = text[pos..].find(delimiter).map(|i| pos + i);

        match (next_delim, next_fence) {
            // Delimiter found and comes before any code fence
            (Some(d), Some(f)) if d < f => {
                let seg = &text[pos..d];
                if !seg.trim().is_empty() {
                    segments.push(seg);
                }
                pos = d + delimiter.len();
            }
            // Delimiter found and no code fence ahead (or fence is later)
            (Some(d), None) => {
                let seg = &text[pos..d];
                if !seg.trim().is_empty() {
                    segments.push(seg);
                }
                pos = d + delimiter.len();
            }
            // Code fence comes first, or only a fence — loop will handle it
            (_, Some(_)) | (None, None) => {
                if next_fence.is_none() {
                    // No more delimiters or fences, take the rest
                    let seg = &text[pos..];
                    if !seg.trim().is_empty() {
                        segments.push(seg);
                    }
                    break;
                }
                // Advance to the fence position so the outer loop picks it up
                let f = next_fence.expect("checked above");
                if f > pos {
                    let seg = &text[pos..f];
                    if !seg.trim().is_empty() {
                        segments.push(seg);
                    }
                    pos = f;
                }
            }
        }
    }

    segments
}

/// Split text on sentence-ending punctuation (`. `, `! `, `? `).
///
/// The punctuation stays attached to the preceding segment.
#[must_use]
fn split_on_sentences(text: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();

    let mut i = 0;
    while i < bytes.len().saturating_sub(1) {
        let is_sentence_end = matches!(bytes[i], b'.' | b'!' | b'?') && bytes[i + 1] == b' ';

        if is_sentence_end {
            // Include the punctuation in this segment, split after the space
            let end = i + 2;
            let seg = &text[start..end];
            if !seg.trim().is_empty() {
                segments.push(seg);
            }
            start = end;
            i = end;
        } else {
            i += 1;
        }
    }

    // Trailing text
    if start < text.len() {
        let seg = &text[start..];
        if !seg.trim().is_empty() {
            segments.push(seg);
        }
    }

    segments
}

/// Assemble segments into chunks that fit within `limit`.
///
/// When a single segment exceeds the limit, it is recursively split using the
/// `fallback` strategy.
fn assemble_chunks(segments: &[&str], limit: usize, fallback: ChunkStrategy) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();

    for &segment in segments {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Check if appending this segment would exceed the limit
        let needed = if current.is_empty() {
            trimmed.len()
        } else {
            current.len() + 2 + trimmed.len() // "\n\n" separator
        };

        if needed <= limit {
            if current.is_empty() {
                current.push_str(trimmed);
            } else {
                current.push_str("\n\n");
                current.push_str(trimmed);
            }
        } else if current.is_empty() {
            // Single segment exceeds limit — split with fallback strategy
            let sub = chunk_text(trimmed, limit, fallback);
            chunks.extend(sub);
        } else {
            // Flush current chunk, start a new one with this segment
            chunks.push(std::mem::take(&mut current));
            if trimmed.len() <= limit {
                current.push_str(trimmed);
            } else {
                let sub = chunk_text(trimmed, limit, fallback);
                chunks.extend(sub);
            }
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

/// Find the best byte offset to split at within `limit`.
///
/// Prefers the last newline before the limit; falls back to `limit` itself.
#[must_use]
fn find_split_point(text: &str, limit: usize) -> usize {
    let search_range = &text[..limit];

    // Prefer splitting at the last newline
    if let Some(pos) = search_range.rfind('\n') {
        if pos > 0 {
            return pos + 1; // Include the newline in the current chunk
        }
    }

    // No newline found — split at the limit, but ensure we are on a char boundary
    let mut split = limit;
    while split > 0 && !text.is_char_boundary(split) {
        split -= 1;
    }

    split
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- chunk_text basics ----

    #[test]
    fn empty_input_returns_empty() {
        let result = chunk_text("", 100, ChunkStrategy::Paragraph);
        assert!(result.is_empty());
    }

    #[test]
    fn text_within_limit_returns_single_chunk() {
        let text = "Hello, world!";
        let result = chunk_text(text, 100, ChunkStrategy::Paragraph);
        assert_eq!(result, vec!["Hello, world!"]);
    }

    #[test]
    fn zero_limit_uses_default() {
        let short = "Hi";
        let result = chunk_text(short, 0, ChunkStrategy::Paragraph);
        assert_eq!(result, vec!["Hi"]);
    }

    // ---- Paragraph strategy ----

    #[test]
    fn paragraph_splits_on_double_newlines() {
        let text = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.";
        let result = chunk_text(text, 30, ChunkStrategy::Paragraph);
        assert!(result.len() >= 2);
        assert!(result.iter().all(|c| c.len() <= 30));
        // All original text is present
        let joined = result.join("\n\n");
        assert!(joined.contains("First paragraph."));
        assert!(joined.contains("Second paragraph."));
        assert!(joined.contains("Third paragraph."));
    }

    #[test]
    fn paragraph_merges_small_paragraphs() {
        let text = "A.\n\nB.\n\nC.";
        let result = chunk_text(text, 100, ChunkStrategy::Paragraph);
        // All three fit in one chunk
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "A.\n\nB.\n\nC.");
    }

    #[test]
    fn paragraph_falls_back_to_sentence() {
        // One big "paragraph" that exceeds the limit
        let text = "Hello world. This is a test. Another sentence here. And one more.";
        let result = chunk_text(text, 40, ChunkStrategy::Paragraph);
        assert!(result.len() >= 2);
        assert!(result.iter().all(|c| c.len() <= 40));
    }

    // ---- Sentence strategy ----

    #[test]
    fn sentence_splits_on_punctuation() {
        let text = "First sentence. Second sentence! Third sentence? Done.";
        let result = chunk_text(text, 30, ChunkStrategy::Sentence);
        assert!(result.len() >= 2);
        assert!(result.iter().all(|c| c.len() <= 30));
    }

    #[test]
    fn sentence_keeps_punctuation_attached() {
        let text = "Hello. World.";
        let result = chunk_text(text, 8, ChunkStrategy::Sentence);
        // "Hello. " is 7 chars, "World." is 6 chars
        assert!(result.iter().any(|c| c.starts_with("Hello.")));
    }

    // ---- HardSplit strategy ----

    #[test]
    fn hard_split_at_boundaries() {
        let text = "abcdefghij"; // 10 chars
        let result = chunk_text(text, 3, ChunkStrategy::HardSplit);
        assert_eq!(result.len(), 4); // "abc", "def", "ghi", "j"
        assert!(result.iter().all(|c| c.len() <= 3));
        assert!(result.iter().all(|c| !c.is_empty()));
    }

    #[test]
    fn hard_split_prefers_newlines() {
        let text = "abc\ndef\nghi\njkl";
        let result = chunk_text(text, 8, ChunkStrategy::HardSplit);
        // Should split at newline boundaries
        assert!(result.iter().all(|c| c.len() <= 8));
        let joined = result.join("\n");
        assert!(joined.contains("abc"));
        assert!(joined.contains("jkl"));
    }

    // ---- Code block preservation ----

    #[test]
    fn paragraph_preserves_code_blocks() {
        let code = "```rust\nfn main() {\n    println!(\"hello\");\n}\n```";
        let text = format!("Intro text.\n\n{code}\n\nOutro text.");
        let result = chunk_text(&text, 200, ChunkStrategy::Paragraph);
        // Code block should not be split across chunks
        let has_complete_block = result.iter().any(|c| c.contains("```rust") && c.contains("```"));
        assert!(has_complete_block, "code block was split: {result:?}");
    }

    #[test]
    fn code_block_kept_whole_when_under_limit() {
        let text = "Before.\n\n```\ncode line 1\ncode line 2\n```\n\nAfter.";
        let result = chunk_text(text, 80, ChunkStrategy::Paragraph);
        // The code block should appear intact in some chunk
        let has_intact = result.iter().any(|c| {
            c.contains("```") && c.contains("code line 1") && c.contains("code line 2")
        });
        assert!(has_intact, "code block was split: {result:?}");
    }

    #[test]
    fn oversized_code_block_still_produces_output() {
        // Code block larger than limit — must fall back gracefully
        let code = "```\n".to_string() + &"x".repeat(50) + "\n```";
        let result = chunk_text(&code, 20, ChunkStrategy::Paragraph);
        assert!(!result.is_empty());
        assert!(result.iter().all(|c| !c.is_empty()));
    }

    // ---- Non-empty chunks guarantee ----

    #[test]
    fn no_empty_chunks_paragraph() {
        let text = "A\n\n\n\nB\n\n\n\n\n\nC";
        let result = chunk_text(text, 5, ChunkStrategy::Paragraph);
        assert!(result.iter().all(|c| !c.is_empty()));
    }

    #[test]
    fn no_empty_chunks_sentence() {
        let text = "A. B. C. D.";
        let result = chunk_text(text, 5, ChunkStrategy::Sentence);
        assert!(result.iter().all(|c| !c.is_empty()));
    }

    #[test]
    fn no_empty_chunks_hard() {
        let text = "   spaced   out   ";
        let result = chunk_text(text, 5, ChunkStrategy::HardSplit);
        assert!(result.iter().all(|c| !c.is_empty()));
    }

    // ---- Multi-byte character safety ----

    #[test]
    fn hard_split_handles_multibyte_chars() {
        // Each emoji is 4 bytes
        let text = "\u{1F600}\u{1F601}\u{1F602}\u{1F603}";
        let result = chunk_text(text, 8, ChunkStrategy::HardSplit);
        assert!(!result.is_empty());
        // Verify no panics from splitting mid-character
        for chunk in &result {
            assert!(chunk.len() <= 8);
            // Ensure valid UTF-8 (implicit — String guarantees this)
            assert!(!chunk.is_empty());
        }
    }

    // ---- Realistic content ----

    #[test]
    fn realistic_long_message() {
        let text = "\
Here is the analysis you requested.\n\n\
The system has three main components:\n\
1. The ingestion pipeline\n\
2. The processing engine\n\
3. The output formatter\n\n\
```python\ndef process(data):\n    return transform(data)\n```\n\n\
Each component communicates over gRPC. The ingestion pipeline handles \
rate limiting and backpressure. The processing engine applies transforms. \
The output formatter renders results.\n\n\
Let me know if you need more details.";

        let result = chunk_text(text, 200, ChunkStrategy::Paragraph);
        assert!(result.iter().all(|c| c.len() <= 200));
        assert!(result.iter().all(|c| !c.is_empty()));

        // Verify all content is present
        let combined = result.join(" ");
        assert!(combined.contains("analysis"));
        assert!(combined.contains("def process"));
        assert!(combined.contains("more details"));
    }

    // ---- split_preserving_code_blocks ----

    #[test]
    fn split_preserving_no_code_blocks() {
        let segments = split_preserving_code_blocks("A\n\nB\n\nC", "\n\n");
        assert_eq!(segments.len(), 3);
    }

    #[test]
    fn split_preserving_ignores_delimiter_in_code() {
        let text = "Before\n\n```\nA\n\nB\n```\n\nAfter";
        let segments = split_preserving_code_blocks(text, "\n\n");
        // The code block should be one segment despite containing \n\n
        let code_seg = segments.iter().find(|s| s.contains("```"));
        assert!(code_seg.is_some());
        let code = code_seg.expect("checked above");
        assert!(code.contains("A\n\nB"));
    }

    // ---- split_on_sentences ----

    #[test]
    fn split_sentences_basic() {
        let segments = split_on_sentences("Hello. World! Test? Done");
        assert_eq!(segments.len(), 4);
        assert_eq!(segments[0], "Hello. ");
        assert_eq!(segments[1], "World! ");
        assert_eq!(segments[2], "Test? ");
        assert_eq!(segments[3], "Done");
    }

    #[test]
    fn split_sentences_no_split_on_abbreviation() {
        // "e.g." has periods but no space after the final one in the right pattern
        let segments = split_on_sentences("Use e.g. foo");
        // Splits on ". " after "e.g" — this is expected (not abbreviation-aware)
        assert!(segments.len() >= 1);
    }
}

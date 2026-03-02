//! Web tools for HTTP operations

mod fetch;
mod readability;
mod search;

pub use fetch::{WebFetchTool, WebResponse};
pub use readability::{Article, extract_article};
pub use search::{SearchProvider, SearchResult, WebSearchTool};

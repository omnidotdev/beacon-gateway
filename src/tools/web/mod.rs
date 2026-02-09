//! Web tools for HTTP operations

mod fetch;
mod readability;
mod search;

pub use fetch::{WebFetchTool, WebResponse};
pub use readability::{extract_article, Article};
pub use search::{SearchProvider, SearchResult, WebSearchTool};

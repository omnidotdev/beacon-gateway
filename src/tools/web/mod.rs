//! Web tools for HTTP operations

mod readability;
mod search;

pub use agent_core::tools::web::fetch::{WebFetchTool, WebResponse};
pub use readability::{Article, extract_article};
pub use search::{SearchProvider, SearchResult, WebSearchTool};

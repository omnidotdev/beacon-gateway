//! Web tools for HTTP operations

mod readability;
pub use agent_core::tools::web::fetch::{WebFetchTool, WebResponse};
pub use agent_core::tools::web::search::{SearchProvider, SearchResult, WebSearchTool};
pub use readability::{Article, extract_article};

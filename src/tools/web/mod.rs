//! Web tools for HTTP operations

pub use agent_core::tools::web::fetch::{WebFetchTool, WebResponse};
pub use agent_core::tools::web::readability::{Article, extract_article};
pub use agent_core::tools::web::search::{SearchProvider, SearchResult, WebSearchTool};

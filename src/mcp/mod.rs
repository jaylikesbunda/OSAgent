//! MCP client support with a deferred tool catalog.
//!
//! MCP servers routinely expose dozens of tools each; loading their
//! schemas into every request is the single largest avoidable context
//! cost in an agent that connects more than one. Instead:
//!
//! 1. `manager` indexes every tool but exposes only a per-server
//!    manifest (one line each) to the model.
//! 2. `tool_search` ranks the catalog lexically (`search`) and
//!    activates matches, appending their schemas to the tool array.
//! 3. `tools::tool_script` lets the agent drive many activated tools
//!    from one sandboxed script, so intermediate results never reach
//!    the transcript at all.

pub mod client;
pub mod manager;
pub mod protocol;
pub mod search;
pub mod transport;

pub use manager::{CatalogEntry, McpHandle, McpManager, ServerSummary, MCP_TOOL_PREFIX};

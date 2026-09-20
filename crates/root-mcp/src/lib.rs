//! Root MCP interface.
//!
//! A local, stdio-only MCP server exposing Root's workspace, work, continuity,
//! and environment primitives to coding agents. MCP is an interface, not the
//! product.

pub mod jsonrpc;
pub mod policy;
pub mod protocol;
pub mod server;
pub mod session;
pub mod tools;

pub use policy::Policy;
pub use server::{
    handle_line, handle_message, run_stdio, serve, status, McpStatusReport, McpWorkspaceSummary,
};
pub use session::ServerState;

#[cfg(test)]
mod tests;

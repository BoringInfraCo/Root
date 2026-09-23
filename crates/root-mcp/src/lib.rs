//! Root MCP interface.
//!
//! `rootd` listens on a Unix socket under the Root directory. `root mcp serve`
//! is the stdio shim agents already launch. Registered capabilities are the
//! workspace, work, continuity, and environment tools. MCP is an interface,
//! not the product.

pub mod daemon;
pub mod jsonrpc;
pub mod policy;
pub mod protocol;
pub mod registry;
pub mod server;
pub mod session;
pub mod tools;

pub use daemon::run_daemon;
pub use policy::Policy;
pub use registry::{
    find as find_capability, registered as capabilities, require as require_capability,
    RegisteredCapability,
};
pub use server::{
    handle_line, handle_message, run_stdio, serve, status, McpStatusReport, McpWorkspaceSummary,
};
pub use session::ServerState;

#[cfg(test)]
mod tests;

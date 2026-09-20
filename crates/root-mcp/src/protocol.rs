//! MCP protocol constants and version negotiation.

pub const PROTOCOL_VERSION: &str = "2024-11-05";
pub const SERVER_NAME: &str = "root";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Root currently supports a single MCP revision. An unknown client request
/// falls back to the supported revision.
pub fn negotiate_protocol_version(_requested: Option<&str>) -> &'static str {
    PROTOCOL_VERSION
}

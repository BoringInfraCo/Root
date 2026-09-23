//! `rootd` — local Unix-socket MCP daemon.
//!
//! Same listener as `root mcp daemon`. Agents keep using `root mcp serve`,
//! which connects here.

fn main() {
    if let Err(error) = root_mcp::run_daemon() {
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}

//! `rootd` — local Unix-socket MCP daemon.
//!
//! Same listener as `root mcp daemon`. Agents keep using `root mcp serve`,
//! which connects here.

fn main() {
    let http = std::env::var("ROOTD_HTTP").ok();
    if let Err(error) = root_mcp::run_daemon(http.as_deref()) {
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}

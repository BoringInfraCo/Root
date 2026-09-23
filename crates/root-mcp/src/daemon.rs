//! Local `rootd` listener.
//!
//! One Unix socket per Root directory. Each connection begins with a single
//! JSON line `{"cwd":"<workspace>"}` and then speaks the same newline-delimited
//! JSON-RPC session as `root mcp serve`. The socket requires the bearer token
//! in `$ROOT_DIR/rootd.token`. Streamable HTTP listens only when asked, and
//! only on 127.0.0.1.

use crate::jsonrpc;
use crate::policy;
use crate::server::run_lines;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const IDLE_EXIT_ENV: &str = "ROOTD_IDLE_EXIT";

#[derive(Debug, Deserialize)]
struct Hello {
    cwd: PathBuf,
    #[serde(default)]
    authorization: Option<String>,
}

pub fn socket_path(root_dir: &Path) -> PathBuf {
    // macOS sun_path is 104 bytes. ROOT_DIR may already be a long temp path,
    // so the socket lives under /tmp and is named from the directory.
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    root_dir.hash(&mut hasher);
    PathBuf::from(format!("/tmp/rootd-{:016x}.sock", hasher.finish()))
}

fn pid_path(root_dir: &Path) -> PathBuf {
    root_dir.join("rootd.pid")
}

/// Listen until the process is stopped. When `ROOTD_IDLE_EXIT=1`, exit after
/// the last connection closes so a shim-started daemon does not outlive the
/// client that spawned it.
pub fn run_daemon(http: Option<&str>) -> Result<()> {
    let root_dir = policy::root_dir()?;
    std::fs::create_dir_all(&root_dir)
        .with_context(|| format!("could not create {}", root_dir.display()))?;
    let token = crate::auth::load_or_create(&root_dir)?;
    if let Some(addr) = http {
        let socket = crate::http::parse_loopback(addr)?;
        let http_listener = std::net::TcpListener::bind(socket)
            .with_context(|| format!("could not bind {socket}"))?;
        let bound = http_listener.local_addr()?;
        std::fs::write(root_dir.join("rootd.http"), bound.to_string())?;
        let cwd = std::env::current_dir()?;
        let http_token = token.clone();
        std::thread::spawn(move || {
            if let Err(error) = crate::http::serve(http_listener, cwd, http_token) {
                eprintln!("rootd http: {error}");
            }
        });
    }
    let path = socket_path(&root_dir);
    let listener = bind_listener(&path)?;
    let _ = std::fs::write(pid_path(&root_dir), std::process::id().to_string());
    let _ = std::fs::write(
        root_dir.join("rootd.path"),
        path.to_string_lossy().as_bytes(),
    );
    let idle_exit = std::env::var_os(IDLE_EXIT_ENV).is_some();
    let connections = Arc::new(AtomicUsize::new(0));

    for accepted in listener.incoming() {
        let stream = match accepted {
            Ok(stream) => stream,
            Err(_) => continue,
        };
        let connections = Arc::clone(&connections);
        connections.fetch_add(1, Ordering::SeqCst);
        let idle = idle_exit;
        let root_for_cleanup = root_dir.clone();
        let token = token.clone();
        std::thread::spawn(move || {
            let _ = serve_connection(stream, &token);
            if idle && connections.fetch_sub(1, Ordering::SeqCst) == 1 {
                cleanup(&root_for_cleanup);
                std::process::exit(0);
            }
        });
    }
    cleanup(&root_dir);
    Ok(())
}

/// Connect to a running rootd, starting an idle-exit daemon if the socket is down.
pub fn connect_stdio_shim() -> Result<UnixStream> {
    let root_dir = policy::root_dir()?;
    std::fs::create_dir_all(&root_dir)?;
    let path = socket_path(&root_dir);
    if let Ok(stream) = UnixStream::connect(&path) {
        return Ok(stream);
    }
    let _ = std::fs::remove_file(&path);
    let mut child = spawn_idle_daemon()?;
    let started = Instant::now();
    loop {
        if let Ok(stream) = UnixStream::connect(&path) {
            if let Some(mut stderr) = child.stderr.take() {
                std::thread::spawn(move || {
                    let mut sink = Vec::new();
                    let _ = std::io::Read::read_to_end(&mut stderr, &mut sink);
                });
            }
            return Ok(stream);
        }
        if let Some(status) = child.try_wait()? {
            let mut err = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                let _ = std::io::Read::read_to_string(&mut stderr, &mut err);
            }
            anyhow::bail!("rootd exited ({status}): {err}");
        }
        if started.elapsed() > Duration::from_secs(3) {
            let mut err = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                let _ = std::io::Read::read_to_string(&mut stderr, &mut err);
            }
            anyhow::bail!(
                "rootd did not accept connections on {}: {err}",
                path.display()
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn spawn_idle_daemon() -> Result<std::process::Child> {
    let exe = std::env::current_exe().context("could not locate the root binary to start rootd")?;
    std::process::Command::new(exe)
        .args(["mcp", "daemon"])
        .env(IDLE_EXIT_ENV, "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to start rootd")
}

fn bind_listener(path: &Path) -> Result<UnixListener> {
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            anyhow::bail!("rootd is already listening on {}", path.display());
        }
        let _ = std::fs::remove_file(path);
    }
    let listener =
        UnixListener::bind(path).with_context(|| format!("could not bind {}", path.display()))?;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(path, permissions)?;
    Ok(listener)
}

fn cleanup(root_dir: &Path) {
    let _ = std::fs::remove_file(socket_path(root_dir));
    let _ = std::fs::remove_file(pid_path(root_dir));
    let _ = std::fs::remove_file(root_dir.join("rootd.path"));
    let _ = std::fs::remove_file(root_dir.join("rootd.http"));
}

fn serve_connection(stream: UnixStream, token: &str) -> Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(());
    }
    let hello: Hello = match serde_json::from_str(line.trim()) {
        Ok(hello) => hello,
        Err(error) => {
            write_error(&mut writer, format!("invalid rootd hello: {error}"))?;
            return Ok(());
        }
    };
    if !crate::auth::bearer_matches(hello.authorization.as_deref(), token) {
        write_error(&mut writer, "unauthorized".to_string())?;
        return Ok(());
    }
    let state = match crate::server::open_session(&hello.cwd) {
        Ok(state) => state,
        Err(error) => {
            write_error(&mut writer, error.to_string())?;
            return Ok(());
        }
    };
    let mut state = state;
    run_lines(&mut state, reader.lines(), &mut writer)
}

fn write_error(writer: &mut UnixStream, message: String) -> Result<()> {
    let response = jsonrpc::error(Value::Null, jsonrpc::INTERNAL_ERROR, message);
    serde_json::to_writer(&mut *writer, &response)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

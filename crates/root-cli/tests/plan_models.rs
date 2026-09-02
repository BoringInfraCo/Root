use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(name: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_cli_plan_models_{name}_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

fn root_bin() -> &'static str {
    env!("CARGO_BIN_EXE_root")
}

#[test]
fn empty_models_exits_0() {
    let dir = tmp("empty");
    std::fs::create_dir_all(&dir).unwrap();
    let output = Command::new(root_bin())
        .args(["plan", "models"])
        .env("ROOT_DIR", &dir)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No declared Ollama models."));
    assert!(stdout.contains("Unsupported operations:"));
    assert!(stdout.contains("This is a preview. No changes have been made."));
    assert!(!dir.join("root.lock").exists());
    assert!(!dir.join("model-pull.json").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_name_exits_2() {
    let dir = tmp("unknown");
    std::fs::create_dir_all(&dir).unwrap();
    let output = Command::new(root_bin())
        .args(["plan", "models", "nope"])
        .env("ROOT_DIR", &dir)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr={} stdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(!dir.join("root.lock").exists());
    assert!(!dir.join("model-pull.json").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn empty_models_json_is_preview() {
    let dir = tmp("empty_json");
    std::fs::create_dir_all(&dir).unwrap();
    let output = Command::new(root_bin())
        .args(["plan", "models", "--json"])
        .env("ROOT_DIR", &dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"command\": \"plan models\""));
    assert!(stdout.contains("\"would_mutate\": false"));
    assert!(stdout.contains("\"protocol\": \"not_probed\""));
    assert!(stdout.contains("\"reason\": \"no_declared_models\""));
    assert!(stdout.contains("digest_addressable_restore"));
    let _ = std::fs::remove_dir_all(&dir);
}

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(name: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_cli_models_pull_{name}_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

fn root_bin() -> &'static str {
    env!("CARGO_BIN_EXE_root")
}

fn write_rootfile(dir: &std::path::Path, body: &str) {
    std::fs::write(dir.join("Rootfile"), body).unwrap();
}

#[test]
fn empty_models_pull_exits_0() {
    let dir = tmp("empty");
    std::fs::create_dir_all(&dir).unwrap();
    let output = Command::new(root_bin())
        .args(["models", "pull"])
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
    assert!(!dir.join("model-pull.json").exists());
    assert!(!dir.join("root.lock").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn empty_models_pull_json_honesty_flags() {
    let dir = tmp("empty_json");
    std::fs::create_dir_all(&dir).unwrap();
    let output = Command::new(root_bin())
        .args(["models", "pull", "--json"])
        .env("ROOT_DIR", &dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"command\": \"models pull\""));
    assert!(stdout.contains("\"models_restored\": false"));
    assert!(stdout.contains("\"model_weights_deleted\": false"));
    assert!(stdout.contains("\"results\": []"));
    assert!(!stdout.contains("\"verb\": \"restored\""));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_name_pull_exits_2() {
    let dir = tmp("unknown");
    std::fs::create_dir_all(&dir).unwrap();
    write_rootfile(&dir, "[models.\"qwen3:8b\"]\nruntime = \"ollama\"\n");
    let output = Command::new(root_bin())
        .args(["models", "pull", "nope"])
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
    assert!(!dir.join("model-pull.json").exists());
    assert!(!dir.join("root.lock").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn policy_deny_pull_exits_9_without_marker() {
    let dir = tmp("policy_deny");
    std::fs::create_dir_all(&dir).unwrap();
    write_rootfile(&dir, "[models.\"qwen3:8b\"]\nruntime = \"ollama\"\n");
    std::fs::write(
        dir.join("policy.toml"),
        "version = 1\n[models]\npull = \"deny\"\n",
    )
    .unwrap();
    let output = Command::new(root_bin())
        .args(["models", "pull"])
        .env("ROOT_DIR", &dir)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(9),
        "stderr={} stdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(!dir.join("model-pull.json").exists());
    assert!(!dir.join("root.lock").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn marker_live_pid_pull_exits_1() {
    let dir = tmp("marker_live");
    std::fs::create_dir_all(&dir).unwrap();
    write_rootfile(&dir, "[models.\"qwen3:8b\"]\nruntime = \"ollama\"\n");
    std::fs::write(
        dir.join("model-pull.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "name": "held",
            "started_at": "2026-09-01T00:00:00Z",
            "pid": std::process::id()
        }))
        .unwrap(),
    )
    .unwrap();
    let output = Command::new(root_bin())
        .args(["models", "pull"])
        .env("ROOT_DIR", &dir)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "stderr={} stdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let kept: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("model-pull.json")).unwrap())
            .unwrap();
    assert_eq!(kept["name"], "held");
    assert!(!dir.join("root.lock").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

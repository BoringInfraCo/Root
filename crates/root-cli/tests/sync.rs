//! Milestone 4: two installations share checkpoint references through a folder.
//! The folder holds ciphertext. No agent is started and Git is not copied.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_sync_{name}_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

fn root_bin() -> &'static str {
    env!("CARGO_BIN_EXE_root")
}

struct Machine {
    repo: PathBuf,
    root_dir: PathBuf,
}

impl Machine {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let repo = base.join("campfire");
        let root_dir = base.join("root");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&root_dir).unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success());
        std::fs::write(repo.join("README.md"), b"source\n").unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["add", "README.md"])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args([
                "-c",
                "user.email=root@example.com",
                "-c",
                "user.name=Root",
                "commit",
                "-q",
                "-m",
                "initial",
            ])
            .status()
            .unwrap()
            .success());
        Self { repo, root_dir }
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let output = Command::new(root_bin())
            .args(args)
            .current_dir(&self.repo)
            .env("ROOT_DIR", &self.root_dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?} stderr={} stdout={}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

#[test]
fn folder_relay_carries_ciphertext_and_a_peer_can_read_it() {
    let origin = Machine::new("origin");
    let peer = Machine::new("peer");
    let relay = tmp("relay");
    std::fs::create_dir_all(&relay).unwrap();

    origin.json(&["workspace", "init", "--json"]);
    origin.json(&["goal", "set", "Ship the portable workspace", "--json"]);
    origin.json(&["checkpoint-sync", "init", "--json"]);
    origin.json(&[
        "checkpoint-sync",
        "relay",
        "set",
        "--folder",
        relay.to_str().unwrap(),
        "--json",
    ]);
    peer.json(&[
        "checkpoint-sync",
        "relay",
        "set",
        "--folder",
        relay.to_str().unwrap(),
        "--json",
    ]);

    let origin_device = origin.json(&["device", "list", "--json"]);
    let peer_device = peer.json(&["device", "list", "--json"]);
    let origin_public = origin.root_dir.join("origin-public.json");
    let peer_public = peer.root_dir.join("peer-public.json");
    std::fs::write(
        &origin_public,
        serde_json::to_vec_pretty(&origin_device["self_device"]).unwrap(),
    )
    .unwrap();
    std::fs::write(
        &peer_public,
        serde_json::to_vec_pretty(&peer_device["self_device"]).unwrap(),
    )
    .unwrap();
    origin.json(&[
        "device",
        "pair",
        "--public",
        peer_public.to_str().unwrap(),
        "--json",
    ]);
    peer.json(&[
        "device",
        "pair",
        "--public",
        origin_public.to_str().unwrap(),
        "--json",
    ]);

    let created = origin.json(&[
        "checkpoint",
        "create",
        "--message",
        "pick up here",
        "--sync",
        "--json",
    ]);
    let checkpoint_id = created["checkpoint"]["id"].as_str().unwrap();

    let mut saw_plaintext = false;
    for entry in walk(&relay) {
        let text = std::fs::read_to_string(&entry).unwrap_or_default();
        if text.contains("pick up here") || text.contains("Ship the portable workspace") {
            saw_plaintext = true;
        }
        assert!(!text.contains("README.md\nsource") && !entry.ends_with("README.md"));
    }
    assert!(!saw_plaintext, "relay stored checkpoint plaintext");

    let pulled = peer.json(&["checkpoint-sync", "pull", "--json"]);
    assert_eq!(pulled["applied"], 1, "{pulled}");
    assert_eq!(pulled["key_installed"], true, "{pulled}");
    let status = peer.json(&["checkpoint-sync", "status", "--json"]);
    assert_eq!(status["inbox"], 1, "{status}");
    assert_eq!(status["conflicts"].as_array().unwrap().len(), 0);
    assert!(status["note"].as_str().unwrap().contains("Git"));

    let state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(peer.root_dir.join("sync/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["inbox"][0]["checkpoint_id"], checkpoint_id);
    assert_eq!(state["inbox"][0]["message"], "pick up here");

    let log = walk(&relay)
        .into_iter()
        .find(|path| path.to_string_lossy().contains("/log/"))
        .unwrap();
    let mut object: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&log).unwrap()).unwrap();
    let cipher = object["ciphertext_hex"].as_str().unwrap().to_string();
    let mut chars: Vec<char> = cipher.chars().collect();
    chars[0] = if chars[0] == 'a' { 'b' } else { 'a' };
    object["ciphertext_hex"] = serde_json::json!(chars.into_iter().collect::<String>());
    std::fs::write(&log, serde_json::to_vec_pretty(&object).unwrap()).unwrap();
    // The good entry is already applied. Add a second broken copy by renaming.
    object["seq"] = serde_json::json!(99);
    let broken = log.with_file_name("0000000000000099-tampered.json");
    std::fs::write(&broken, serde_json::to_vec_pretty(&object).unwrap()).unwrap();
    // Restore the original so the applied file stays valid, rejection is the new file.
    object["ciphertext_hex"] = serde_json::json!(cipher);
    std::fs::write(&log, serde_json::to_vec_pretty(&object).unwrap()).unwrap();

    let again = peer.json(&["checkpoint-sync", "pull", "--json"]);
    assert!(again["rejected"].as_u64().unwrap() >= 1, "{again}");
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

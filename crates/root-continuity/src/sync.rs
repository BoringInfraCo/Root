//! Encrypted checkpoint sync over a folder relay.
//!
//! The folder stores ciphertext, workspace ids, device ids, sizes, and timestamps.
//! The sync key never leaves a device except sealed to a paired device. Credential
//! values are not written. Git remains the source transport. Conflicts are recorded
//! and not merged.

use anyhow::{Context, Result};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use chrono::Utc;
use crypto_box::{PublicKey, SalsaBox, SecretKey};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const GIT_NOTE: &str = "Git still carries source code. Sync carries checkpoint references only.";

#[derive(Debug, Serialize, Deserialize)]
struct DeviceFile {
    device_id: String,
    sign_secret_hex: String,
    sign_public_hex: String,
    box_secret_hex: String,
    box_public_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicDevice {
    pub device_id: String,
    pub sign_public_hex: String,
    pub box_public_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Peer {
    device_id: String,
    sign_public_hex: String,
    box_public_hex: String,
    revoked: bool,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct PeerFile {
    peers: Vec<Peer>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct SyncState {
    workspace_id: Option<String>,
    relay: Option<String>,
    pushed: Vec<String>,
    applied: Vec<u64>,
    inbox: Vec<InboxEntry>,
    conflicts: Vec<Conflict>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InboxEntry {
    seq: u64,
    device_id: String,
    checkpoint_id: String,
    work_revision: i64,
    message: String,
    git_head: Option<String>,
    agent_env_sha256: String,
    credential_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    work_revision: i64,
    left: String,
    right: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Envelope {
    kind: String,
    checkpoint_id: String,
    workspace_id: String,
    work_revision: i64,
    git_head: Option<String>,
    message: String,
    agent_env_sha256: String,
    credential_names: Vec<String>,
    device_id: String,
    payload_sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct RelayObject {
    workspace_id: String,
    device_id: String,
    seq: u64,
    nonce_hex: String,
    ciphertext_hex: String,
    signature_hex: String,
    size: usize,
    created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WrappedKey {
    workspace_id: String,
    recipient_device_id: String,
    sender_device_id: String,
    sender_box_public_hex: String,
    nonce_hex: String,
    ciphertext_hex: String,
    size: usize,
    created_at: String,
}

#[derive(Debug, Serialize)]
pub struct DeviceList {
    pub self_device: PublicDevice,
    pub peers: Vec<PublicDevice>,
    pub revoked: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SyncStatus {
    pub device_id: String,
    pub workspace_id: Option<String>,
    pub peers: usize,
    pub revoked: usize,
    pub relay: Option<String>,
    pub pushed: usize,
    pub inbox: usize,
    pub conflicts: Vec<Conflict>,
    pub note: &'static str,
}

#[derive(Debug, Serialize)]
pub struct PushReport {
    pub pushed: usize,
    pub workspace_id: String,
}

#[derive(Debug, Serialize)]
pub struct PullReport {
    pub applied: usize,
    pub rejected: usize,
    pub conflicts: usize,
    pub key_installed: bool,
}

pub fn device_list() -> Result<DeviceList> {
    let device = ensure_device()?;
    let peers = read_peers()?;
    Ok(DeviceList {
        self_device: device.public(),
        revoked: peers
            .peers
            .iter()
            .filter(|peer| peer.revoked)
            .map(|peer| peer.device_id.clone())
            .collect(),
        peers: peers
            .peers
            .iter()
            .filter(|peer| !peer.revoked)
            .map(|peer| PublicDevice {
                device_id: peer.device_id.clone(),
                sign_public_hex: peer.sign_public_hex.clone(),
                box_public_hex: peer.box_public_hex.clone(),
            })
            .collect(),
    })
}

pub fn device_pair(public_path: &Path) -> Result<PublicDevice> {
    let text = fs::read_to_string(public_path)
        .with_context(|| format!("could not read {}", public_path.display()))?;
    let public: PublicDevice = serde_json::from_str(&text).context("invalid device public file")?;
    let self_id = ensure_device()?.device_id;
    if public.device_id == self_id {
        anyhow::bail!("cannot pair a device with itself");
    }
    let mut peers = read_peers()?;
    if let Some(existing) = peers
        .peers
        .iter_mut()
        .find(|peer| peer.device_id == public.device_id)
    {
        existing.sign_public_hex = public.sign_public_hex.clone();
        existing.box_public_hex = public.box_public_hex.clone();
        existing.revoked = false;
    } else {
        peers.peers.push(Peer {
            device_id: public.device_id.clone(),
            sign_public_hex: public.sign_public_hex.clone(),
            box_public_hex: public.box_public_hex.clone(),
            revoked: false,
        });
    }
    write_peers(&peers)?;
    if sync_key_exists() {
        if let Some(relay) = read_state()?.relay {
            publish_wrapped_keys(Path::new(&relay))?;
        }
    }
    Ok(public)
}

pub fn device_revoke(device_id: &str) -> Result<PublicDevice> {
    let mut peers = read_peers()?;
    let Some(peer) = peers
        .peers
        .iter_mut()
        .find(|peer| peer.device_id == device_id)
    else {
        anyhow::bail!("unknown device {device_id}");
    };
    peer.revoked = true;
    let view = PublicDevice {
        device_id: peer.device_id.clone(),
        sign_public_hex: peer.sign_public_hex.clone(),
        box_public_hex: peer.box_public_hex.clone(),
    };
    write_peers(&peers)?;
    Ok(view)
}

pub fn sync_init(cwd: &Path) -> Result<SyncStatus> {
    let _device = ensure_device()?;
    let workspace_id = workspace_id(cwd)?;
    let mut state = read_state()?;
    if let Some(existing) = &state.workspace_id {
        if existing != &workspace_id {
            anyhow::bail!("this installation already syncs workspace {existing}");
        }
    }
    state.workspace_id = Some(workspace_id);
    write_state(&state)?;
    if !sync_key_exists() {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        write_secret(&sync_dir()?.join("key.bin"), &key)?;
    }
    status()
}

pub fn relay_set(folder: &Path) -> Result<SyncStatus> {
    if !folder.is_dir() {
        anyhow::bail!("relay folder does not exist: {}", folder.display());
    }
    let mut state = read_state()?;
    state.relay = Some(folder.display().to_string());
    write_state(&state)?;
    status()
}

pub fn relay_show() -> Result<SyncStatus> {
    status()
}

pub fn status() -> Result<SyncStatus> {
    let device = ensure_device()?;
    let peers = read_peers()?;
    let state = read_state()?;
    Ok(SyncStatus {
        device_id: device.device_id,
        workspace_id: state.workspace_id,
        peers: peers.peers.iter().filter(|peer| !peer.revoked).count(),
        revoked: peers.peers.iter().filter(|peer| peer.revoked).count(),
        relay: state.relay,
        pushed: state.pushed.len(),
        inbox: state.inbox.len(),
        conflicts: state.conflicts,
        note: GIT_NOTE,
    })
}

pub fn push(cwd: &Path) -> Result<PushReport> {
    let device = ensure_device()?;
    let key = read_sync_key()?;
    let state = read_state()?;
    let workspace = state
        .workspace_id
        .clone()
        .context("sync is not initialized")?;
    let relay = state.relay.clone().context("sync relay is not set")?;
    let relay = PathBuf::from(relay);
    publish_wrapped_keys(&relay)?;
    let report = crate::list(cwd)?;
    if report.checkpoints.first().map(|cp| &cp.workspace_id) != Some(&workspace)
        && !report.checkpoints.is_empty()
    {
        anyhow::bail!("checkpoints belong to a different workspace than sync init");
    }
    let mut ordered = report.checkpoints;
    ordered.sort_by_key(|cp| cp.work_revision);
    let mut pushed = 0;
    for checkpoint in ordered {
        if state_contains(&checkpoint.id)? {
            continue;
        }
        append_checkpoint(&device, &key, &workspace, &relay, &checkpoint)?;
        pushed += 1;
    }
    Ok(PushReport {
        pushed,
        workspace_id: workspace,
    })
}

pub fn pull() -> Result<PullReport> {
    let device = ensure_device()?;
    let mut state = read_state()?;
    let relay = state.relay.clone().context("sync relay is not set")?;
    let relay = PathBuf::from(&relay);
    if state.workspace_id.is_none() {
        state.workspace_id = Some(discover_workspace(&device, &relay)?);
        write_state(&state)?;
    }
    let workspace = state.workspace_id.clone().unwrap();
    let mut key_installed = false;
    if !sync_key_exists() {
        install_wrapped_key(&device, &relay, &workspace)?;
        key_installed = true;
    }
    let key = read_sync_key()?;
    let peers = read_peers()?;
    let mut applied = 0;
    let mut rejected = 0;
    let dir = relay.join(&workspace).join("log");
    if !dir.exists() {
        return Ok(PullReport {
            applied: 0,
            rejected: 0,
            conflicts: state.conflicts.len(),
            key_installed,
        });
    }
    let mut files: Vec<_> = fs::read_dir(&dir)?.filter_map(|entry| entry.ok()).collect();
    files.sort_by_key(|entry| entry.file_name());
    for entry in files {
        let text = fs::read_to_string(entry.path())?;
        let object: RelayObject = match serde_json::from_str(&text) {
            Ok(object) => object,
            Err(_) => {
                rejected += 1;
                continue;
            }
        };
        if state.applied.contains(&object.seq) {
            continue;
        }
        if peers
            .peers
            .iter()
            .any(|peer| peer.device_id == object.device_id && peer.revoked)
        {
            rejected += 1;
            continue;
        }
        let verifying = if object.device_id == device.device_id {
            device.verifying_key()?
        } else if let Some(peer) = peers
            .peers
            .iter()
            .find(|peer| peer.device_id == object.device_id && !peer.revoked)
        {
            peer_verifying(peer)?
        } else {
            rejected += 1;
            continue;
        };
        if verify_object(&object, &verifying).is_err() {
            rejected += 1;
            continue;
        }
        let plain = match decrypt_object(&object, &key) {
            Ok(plain) => plain,
            Err(_) => {
                rejected += 1;
                continue;
            }
        };
        note_conflict(&mut state, &plain);
        state.inbox.push(InboxEntry {
            seq: object.seq,
            device_id: object.device_id,
            checkpoint_id: plain.checkpoint_id,
            work_revision: plain.work_revision,
            message: plain.message,
            git_head: plain.git_head,
            agent_env_sha256: plain.agent_env_sha256,
            credential_names: plain.credential_names,
        });
        state.applied.push(object.seq);
        applied += 1;
    }
    let conflicts = state.conflicts.len();
    write_state(&state)?;
    Ok(PullReport {
        applied,
        rejected,
        conflicts,
        key_installed,
    })
}

fn append_checkpoint(
    device: &DeviceFile,
    key: &[u8; 32],
    workspace: &str,
    relay: &Path,
    checkpoint: &root_work::CheckpointSummary,
) -> Result<()> {
    let (message, agent_env) = withhold(&checkpoint.message, &checkpoint.agent_env_ref);
    let names = credential_names(agent_env.as_deref());
    let agent_hash = sha256_hex(agent_env.unwrap_or_default().as_bytes());
    let mut envelope = Envelope {
        kind: "checkpoint".to_string(),
        checkpoint_id: checkpoint.id.clone(),
        workspace_id: workspace.to_string(),
        work_revision: checkpoint.work_revision,
        git_head: checkpoint.git_head.clone(),
        message,
        agent_env_sha256: agent_hash,
        credential_names: names,
        device_id: device.device_id.clone(),
        payload_sha256: String::new(),
    };
    let without = serde_json::to_vec(&envelope)?;
    envelope.payload_sha256 = sha256_hex(&without);
    let plain = serde_json::to_vec(&envelope)?;
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let cipher = ChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(key));
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plain.as_ref())
        .map_err(|_| anyhow::anyhow!("could not encrypt checkpoint"))?;
    let seq = next_seq(relay, workspace)?;
    let ciphertext_hex = hex::encode(&ciphertext);
    let signed = format!("{workspace}:{seq}:{ciphertext_hex}");
    let signature = device.signing_key()?.sign(signed.as_bytes());
    let object = RelayObject {
        workspace_id: workspace.to_string(),
        device_id: device.device_id.clone(),
        seq,
        nonce_hex: hex::encode(nonce),
        ciphertext_hex,
        signature_hex: hex::encode(signature.to_bytes()),
        size: ciphertext.len(),
        created_at: Utc::now().to_rfc3339(),
    };
    let dir = relay.join(workspace).join("log");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{seq:016}-{}.json", device.device_id));
    fs::write(path, serde_json::to_vec_pretty(&object)?)?;
    let mut state = read_state()?;
    state.pushed.push(checkpoint.id.clone());
    write_state(&state)?;
    Ok(())
}

fn publish_wrapped_keys(relay: &Path) -> Result<()> {
    let Some(workspace) = read_state()?.workspace_id else {
        return Ok(());
    };
    let device = ensure_device()?;
    let key = read_sync_key()?;
    let peers = read_peers()?;
    for peer in peers.peers.iter().filter(|peer| !peer.revoked) {
        let recipient = PublicKey::from(decode32(&peer.box_public_hex)?);
        let (nonce_hex, ciphertext_hex) = seal(&device, &recipient, &key)?;
        let wrapped = WrappedKey {
            workspace_id: workspace.clone(),
            recipient_device_id: peer.device_id.clone(),
            sender_device_id: device.device_id.clone(),
            sender_box_public_hex: device.box_public_hex.clone(),
            nonce_hex,
            ciphertext_hex: ciphertext_hex.clone(),
            size: ciphertext_hex.len() / 2,
            created_at: Utc::now().to_rfc3339(),
        };
        let dir = relay.join(&workspace).join("keys");
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join(format!("{}.json", peer.device_id)),
            serde_json::to_vec_pretty(&wrapped)?,
        )?;
    }
    Ok(())
}

fn discover_workspace(device: &DeviceFile, relay: &Path) -> Result<String> {
    if !relay.exists() {
        anyhow::bail!("relay folder does not exist");
    }
    for entry in fs::read_dir(relay)? {
        let path = entry?
            .path()
            .join("keys")
            .join(format!("{}.json", device.device_id));
        if !path.exists() {
            continue;
        }
        let wrapped: WrappedKey = serde_json::from_str(&fs::read_to_string(&path)?)?;
        return Ok(wrapped.workspace_id);
    }
    anyhow::bail!("no wrapped sync key for this device is in the relay")
}

fn install_wrapped_key(device: &DeviceFile, relay: &Path, workspace: &str) -> Result<()> {
    let path = relay
        .join(workspace)
        .join("keys")
        .join(format!("{}.json", device.device_id));
    let text = fs::read_to_string(&path)
        .with_context(|| format!("no wrapped sync key at {}", path.display()))?;
    let wrapped: WrappedKey = serde_json::from_str(&text)?;
    let sender = PublicKey::from(decode32(&wrapped.sender_box_public_hex)?);
    let secret = SecretKey::from(decode32(&device.box_secret_hex)?);
    let nonce = decode_hex(&wrapped.nonce_hex)?;
    let ciphertext = decode_hex(&wrapped.ciphertext_hex)?;
    let salsa = SalsaBox::new(&sender, &secret);
    let nonce = crypto_box::Nonce::from_slice(&nonce);
    let plain = salsa
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|_| anyhow::anyhow!("could not open sync key"))?;
    if plain.len() != 32 {
        anyhow::bail!("sync key has the wrong length");
    }
    write_secret(&sync_dir()?.join("key.bin"), &plain)?;
    Ok(())
}

fn seal(device: &DeviceFile, recipient: &PublicKey, key: &[u8; 32]) -> Result<(String, String)> {
    let secret = SecretKey::from(decode32(&device.box_secret_hex)?);
    let salsa = SalsaBox::new(recipient, &secret);
    let mut nonce = [0u8; 24];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = salsa
        .encrypt(crypto_box::Nonce::from_slice(&nonce), key.as_ref())
        .map_err(|_| anyhow::anyhow!("could not seal sync key"))?;
    Ok((hex::encode(nonce), hex::encode(ciphertext)))
}

fn verify_object(object: &RelayObject, key: &VerifyingKey) -> Result<()> {
    let signed = format!(
        "{}:{}:{}",
        object.workspace_id, object.seq, object.ciphertext_hex
    );
    let bytes = decode_hex(&object.signature_hex)?;
    let signature = Signature::from_slice(&bytes).context("bad signature length")?;
    key.verify(signed.as_bytes(), &signature)
        .map_err(|_| anyhow::anyhow!("signature verification failed"))?;
    Ok(())
}

fn decrypt_object(object: &RelayObject, key: &[u8; 32]) -> Result<Envelope> {
    let cipher = ChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(key));
    let nonce = decode_hex(&object.nonce_hex)?;
    let ciphertext = decode_hex(&object.ciphertext_hex)?;
    let plain = cipher
        .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|_| anyhow::anyhow!("could not decrypt checkpoint"))?;
    let envelope: Envelope = serde_json::from_slice(&plain)?;
    let mut check = envelope.clone();
    check.payload_sha256.clear();
    let expected = sha256_hex(&serde_json::to_vec(&check)?);
    if expected != envelope.payload_sha256 {
        anyhow::bail!("payload hash mismatch");
    }
    Ok(envelope)
}

fn note_conflict(state: &mut SyncState, incoming: &Envelope) {
    if let Some(previous) = state.inbox.iter().find(|entry| {
        entry.work_revision == incoming.work_revision
            && entry.checkpoint_id != incoming.checkpoint_id
    }) {
        state.conflicts.push(Conflict {
            work_revision: incoming.work_revision,
            left: previous.checkpoint_id.clone(),
            right: incoming.checkpoint_id.clone(),
        });
    }
}

fn withhold(message: &Option<String>, agent_env: &Option<String>) -> (String, Option<String>) {
    let message = match message {
        Some(text) if root_work::secrets::detect(text).is_some() => "[withheld]".to_string(),
        Some(text) => text.clone(),
        None => String::new(),
    };
    let agent_env = match agent_env {
        Some(text) if root_work::secrets::detect(text).is_some() => None,
        other => other.clone(),
    };
    (message, agent_env)
}

fn credential_names(agent_env: Option<&str>) -> Vec<String> {
    let Some(text) = agent_env else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    collect_names(&value, &mut names);
    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter(|name| root_work::secrets::detect(name).is_none())
        .collect()
}

fn collect_names(value: &Value, names: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if key == "name" || key.ends_with("_name") || key == "credential_refs" {
                    if let Some(text) = child.as_str() {
                        names.push(text.to_string());
                    }
                }
                collect_names(child, names);
            }
        }
        Value::Array(items) => {
            for item in items {
                if let Some(text) = item.as_str() {
                    if value_key_is_name(item) {
                        names.push(text.to_string());
                    }
                }
                collect_names(item, names);
            }
        }
        _ => {}
    }
}

fn value_key_is_name(_value: &Value) -> bool {
    false
}

fn next_seq(relay: &Path, workspace: &str) -> Result<u64> {
    let dir = relay.join(workspace).join("log");
    if !dir.exists() {
        return Ok(1);
    }
    let mut max = 0;
    for entry in fs::read_dir(dir)? {
        let name = entry?.file_name();
        let name = name.to_string_lossy();
        if let Some(prefix) = name.split('-').next() {
            if let Ok(seq) = prefix.parse::<u64>() {
                max = max.max(seq);
            }
        }
    }
    Ok(max + 1)
}

fn state_contains(checkpoint_id: &str) -> Result<bool> {
    Ok(read_state()?.pushed.iter().any(|id| id == checkpoint_id))
}

fn workspace_id(cwd: &Path) -> Result<String> {
    let repository = root_work::Repository::discover(cwd)?;
    let store = root_work::WorkStore::open(repository)?;
    Ok(store.workspace().id.clone())
}

impl DeviceFile {
    fn public(&self) -> PublicDevice {
        PublicDevice {
            device_id: self.device_id.clone(),
            sign_public_hex: self.sign_public_hex.clone(),
            box_public_hex: self.box_public_hex.clone(),
        }
    }

    fn signing_key(&self) -> Result<SigningKey> {
        let bytes = decode32(&self.sign_secret_hex)?;
        Ok(SigningKey::from_bytes(&bytes))
    }

    fn verifying_key(&self) -> Result<VerifyingKey> {
        let bytes = decode32(&self.sign_public_hex)?;
        VerifyingKey::from_bytes(&bytes).map_err(|_| anyhow::anyhow!("bad verifying key"))
    }
}

fn ensure_device() -> Result<DeviceFile> {
    let path = sync_dir()?.join("device.json");
    if path.exists() {
        let text = fs::read_to_string(path)?;
        return serde_json::from_str(&text).context("invalid device file");
    }
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    let signing = SigningKey::from_bytes(&seed);
    let verifying = signing.verifying_key();
    let boxing = SecretKey::generate(&mut OsRng);
    let boxing_public = boxing.public_key();
    let sign_public = hex::encode(verifying.to_bytes());
    let device = DeviceFile {
        device_id: sha256_hex(sign_public.as_bytes())[..16].to_string(),
        sign_secret_hex: hex::encode(signing.to_bytes()),
        sign_public_hex: sign_public,
        box_secret_hex: hex::encode(boxing.to_bytes()),
        box_public_hex: hex::encode(boxing_public.as_bytes()),
    };
    write_secret_text(&path, &serde_json::to_string_pretty(&device)?)?;
    let text = fs::read_to_string(sync_dir()?.join("device.json"))?;
    serde_json::from_str(&text).context("invalid device file")
}

fn peer_verifying(peer: &Peer) -> Result<VerifyingKey> {
    let bytes = decode32(&peer.sign_public_hex)?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| anyhow::anyhow!("bad peer key"))
}

fn sync_key_exists() -> bool {
    sync_dir()
        .map(|dir| dir.join("key.bin").exists())
        .unwrap_or(false)
}

fn read_sync_key() -> Result<[u8; 32]> {
    let bytes = fs::read(sync_dir()?.join("key.bin")).context("sync key is missing")?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("sync key has the wrong length"))?;
    Ok(bytes)
}

fn read_peers() -> Result<PeerFile> {
    let path = sync_dir()?.join("peers.json");
    if !path.exists() {
        return Ok(PeerFile::default());
    }
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

fn write_peers(peers: &PeerFile) -> Result<()> {
    write_secret_text(
        &sync_dir()?.join("peers.json"),
        &serde_json::to_string_pretty(peers)?,
    )
}

fn read_state() -> Result<SyncState> {
    let path = sync_dir()?.join("state.json");
    if !path.exists() {
        return Ok(SyncState::default());
    }
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

fn write_state(state: &SyncState) -> Result<()> {
    fs::write(
        sync_dir()?.join("state.json"),
        serde_json::to_vec_pretty(state)?,
    )?;
    Ok(())
}

fn sync_dir() -> Result<PathBuf> {
    let dir = root_lockfile::get_root_dir()?.join("sync");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn write_secret(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

fn write_secret_text(path: &Path, text: &str) -> Result<()> {
    write_secret(path, text.as_bytes())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn decode_hex(text: &str) -> Result<Vec<u8>> {
    hex::decode(text).context("invalid hex")
}

fn decode32(text: &str) -> Result<[u8; 32]> {
    let bytes = decode_hex(text)?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("expected 32 bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_revision_from_two_checkpoints_is_a_conflict() {
        let mut state = SyncState::default();
        state.inbox.push(InboxEntry {
            seq: 1,
            device_id: "aaaa".into(),
            checkpoint_id: "root_cp_a".into(),
            work_revision: 2,
            message: String::new(),
            git_head: None,
            agent_env_sha256: String::new(),
            credential_names: Vec::new(),
        });
        let incoming = Envelope {
            kind: "checkpoint".into(),
            checkpoint_id: "root_cp_b".into(),
            workspace_id: "ws".into(),
            work_revision: 2,
            git_head: None,
            message: String::new(),
            agent_env_sha256: String::new(),
            credential_names: Vec::new(),
            device_id: "bbbb".into(),
            payload_sha256: String::new(),
        };
        note_conflict(&mut state, &incoming);
        assert_eq!(state.conflicts.len(), 1);
        assert_eq!(state.conflicts[0].left, "root_cp_a");
        assert_eq!(state.conflicts[0].right, "root_cp_b");
    }
}

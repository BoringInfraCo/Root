//! Read-only finance projection, plus payment intents.
//!
//! The installed fixture files are the ledger. Intents are a separate record.
//! Approving an intent does not execute it and does not move money.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const MAX_AMOUNT_CENTS: u64 = 10_000_000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransactionFile {
    currency: String,
    transactions: Vec<Transaction>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Transaction {
    id: String,
    posted_on: String,
    payee: String,
    amount_cents: u64,
    category: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptFile {
    receipts: Vec<Receipt>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    id: String,
    payee: String,
    amount_cents: u64,
}

#[derive(Debug, Serialize)]
struct TransactionReport {
    currency: String,
    transactions: Vec<Transaction>,
}

#[derive(Debug, Serialize)]
struct ReceiptReport {
    receipts: Vec<Receipt>,
}

#[derive(Debug, Serialize)]
struct ReconcileReport {
    matched: Vec<Match>,
    unmatched_transactions: Vec<String>,
    unmatched_receipts: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Match {
    transaction_id: String,
    receipt_id: String,
}

struct Ledger {
    currency: String,
    transactions: Vec<Transaction>,
    receipts: Vec<Receipt>,
}

pub(crate) fn screen_package(dir: &Path) -> Result<()> {
    let _ = load_dir(dir)?;
    Ok(())
}

pub(crate) fn project(connector_id: &str, tool: &str) -> Result<String> {
    let ledger = load_dir(&installed_files(connector_id)?)?;
    let value = match tool.rsplit('.').next() {
        Some("transactions") => serde_json::to_value(TransactionReport {
            currency: ledger.currency,
            transactions: ledger.transactions,
        })?,
        Some("receipts") => serde_json::to_value(ReceiptReport {
            receipts: ledger.receipts,
        })?,
        Some("reconcile") => serde_json::to_value(reconcile(&ledger))?,
        _ => anyhow::bail!("unknown finance tool {tool}"),
    };
    Ok(value.to_string())
}

fn reconcile(ledger: &Ledger) -> ReconcileReport {
    let mut used = BTreeSet::new();
    let mut matched = Vec::new();
    let mut unmatched_receipts = Vec::new();
    for receipt in &ledger.receipts {
        let Some(transaction) = ledger.transactions.iter().find(|transaction| {
            !used.contains(&transaction.id)
                && transaction.payee == receipt.payee
                && transaction.amount_cents == receipt.amount_cents
        }) else {
            unmatched_receipts.push(receipt.id.clone());
            continue;
        };
        used.insert(transaction.id.clone());
        matched.push(Match {
            transaction_id: transaction.id.clone(),
            receipt_id: receipt.id.clone(),
        });
    }
    let unmatched_transactions = ledger
        .transactions
        .iter()
        .filter(|transaction| !used.contains(&transaction.id))
        .map(|transaction| transaction.id.clone())
        .collect();
    ReconcileReport {
        matched,
        unmatched_transactions,
        unmatched_receipts,
    }
}

fn installed_files(connector_id: &str) -> Result<PathBuf> {
    let root = crate::policy::root_dir()?;
    let dir = root.join("connectors").join(connector_id).join("files");
    if !dir.is_dir() {
        anyhow::bail!("finance ledger is not installed");
    }
    Ok(dir)
}

fn load_dir(dir: &Path) -> Result<Ledger> {
    let transactions = read_ledger_file(&dir.join("transactions.json"))?;
    let receipts = read_ledger_file(&dir.join("receipts.json"))?;
    let transactions: TransactionFile =
        serde_json::from_str(&transactions).context("invalid transactions.json")?;
    let receipts: ReceiptFile = serde_json::from_str(&receipts).context("invalid receipts.json")?;
    check_currency(&transactions.currency)?;
    let mut ids = BTreeSet::new();
    for transaction in &transactions.transactions {
        check_row(
            &transaction.id,
            &transaction.posted_on,
            &transaction.payee,
            transaction.amount_cents,
            Some(&transaction.category),
        )?;
        if !ids.insert(transaction.id.clone()) {
            anyhow::bail!("duplicate transaction id");
        }
    }
    for receipt in &receipts.receipts {
        check_row(&receipt.id, "", &receipt.payee, receipt.amount_cents, None)?;
        if !ids.insert(receipt.id.clone()) {
            anyhow::bail!("duplicate receipt id");
        }
    }
    Ok(Ledger {
        currency: transactions.currency,
        transactions: transactions.transactions,
        receipts: receipts.receipts,
    })
}

fn read_ledger_file(path: &Path) -> Result<String> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    if root_work::secrets::detect(&text).is_some() || digit_run(&text, 13) {
        anyhow::bail!("refusing a card or bank number");
    }
    Ok(text)
}

fn check_currency(currency: &str) -> Result<()> {
    if currency.len() == 3 && currency.chars().all(|c| c.is_ascii_uppercase()) {
        Ok(())
    } else {
        anyhow::bail!("currency must be a 3-letter code")
    }
}

fn check_row(
    id: &str,
    posted_on: &str,
    payee: &str,
    amount_cents: u64,
    category: Option<&str>,
) -> Result<()> {
    if !valid_id(id) || digit_run(id, 4) {
        anyhow::bail!("invalid ledger id");
    }
    if !posted_on.is_empty() && !valid_date(posted_on) {
        anyhow::bail!("invalid posted_on");
    }
    if !valid_payee(payee) {
        anyhow::bail!("invalid payee");
    }
    if amount_cents > MAX_AMOUNT_CENTS {
        anyhow::bail!("amount is outside the ledger range");
    }
    if let Some(category) = category {
        if !valid_category(category) {
            anyhow::bail!("invalid category");
        }
    }
    Ok(())
}

fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && id.len() <= 16
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..].iter().all(u8::is_ascii_digit)
}

fn valid_payee(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && value.len() <= 32
        && value
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c.is_ascii_digit() || c == ' ' || c == '-')
        && !digit_run(value, 4)
}

fn valid_category(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && value.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c == ' ')
}

/// Fixture provider cap for a payment intent. This is not a bank limit.
pub const PROVIDER_LIMIT_CENTS: u64 = 25_000;

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct IntentFile {
    intents: Vec<Intent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    id: String,
    amount_cents: u64,
    currency: String,
    recipient: String,
    purpose: String,
    idempotency_key: String,
    status: String,
    created_at: String,
    approved_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IntentView {
    pub id: String,
    pub amount_cents: u64,
    pub currency: String,
    pub recipient: String,
    pub purpose: String,
    pub idempotency_key: String,
    pub status: String,
    pub created_at: String,
    pub approved_at: Option<String>,
}

pub fn create_intent(
    amount_cents: u64,
    recipient: &str,
    purpose: &str,
    idempotency_key: &str,
) -> Result<IntentView> {
    check_intent_fields(amount_cents, recipient, purpose, idempotency_key)?;
    let _lock = lock_intents()?;
    let mut file = read_intents()?;
    if let Some(existing) = file
        .intents
        .iter()
        .find(|intent| intent.idempotency_key == idempotency_key)
    {
        if existing.amount_cents == amount_cents
            && existing.recipient == recipient
            && existing.purpose == purpose
        {
            return Ok(intent_view(existing));
        }
        anyhow::bail!("idempotency key already used");
    }
    let intent = Intent {
        id: format!("root_fi_{}", crate::auth::random_hex(8)?),
        amount_cents,
        currency: "USD".to_string(),
        recipient: recipient.to_string(),
        purpose: purpose.to_string(),
        idempotency_key: idempotency_key.to_string(),
        status: "pending".to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        approved_at: None,
    };
    let view = intent_view(&intent);
    file.intents.push(intent);
    write_intents(&file)?;
    Ok(view)
}

pub fn list_intents() -> Result<Vec<IntentView>> {
    Ok(read_intents()?.intents.iter().map(intent_view).collect())
}

pub fn approve_intent(id: &str) -> Result<IntentView> {
    let path = crate::policy::root_dir()?
        .join("finance")
        .join("intents.json");
    if !path.is_file() {
        anyhow::bail!("unknown payment intent {id}");
    }
    let _lock = lock_intents()?;
    let mut file = read_intents()?;
    let Some(intent) = file.intents.iter_mut().find(|intent| intent.id == id) else {
        anyhow::bail!("unknown payment intent {id}");
    };
    if intent.status == "approved" {
        return Ok(intent_view(intent));
    }
    intent.status = "approved".to_string();
    intent.approved_at = Some(chrono::Utc::now().to_rfc3339());
    let view = intent_view(intent);
    write_intents(&file)?;
    Ok(view)
}

fn check_intent_fields(
    amount_cents: u64,
    recipient: &str,
    purpose: &str,
    idempotency_key: &str,
) -> Result<()> {
    if amount_cents == 0 {
        anyhow::bail!("amount must be at least 1 cent");
    }
    if amount_cents > PROVIDER_LIMIT_CENTS {
        anyhow::bail!("amount exceeds the provider limit of {PROVIDER_LIMIT_CENTS} cents");
    }
    if root_work::secrets::detect(recipient).is_some() || digit_run(recipient, 4) {
        anyhow::bail!("refusing a card or bank number");
    }
    if !valid_payee(recipient) {
        anyhow::bail!("invalid recipient");
    }
    if root_work::secrets::detect(purpose).is_some() || digit_run(purpose, 4) {
        anyhow::bail!("refusing a card or bank number");
    }
    if !valid_purpose(purpose) {
        anyhow::bail!("invalid purpose");
    }
    if !valid_idempotency_key(idempotency_key)
        || root_work::secrets::detect(idempotency_key).is_some()
    {
        anyhow::bail!("invalid idempotency key");
    }
    Ok(())
}

fn valid_purpose(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-')
        && !digit_run(value, 13)
}

fn valid_idempotency_key(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && value.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && !digit_run(value, 13)
}

fn intent_view(intent: &Intent) -> IntentView {
    IntentView {
        id: intent.id.clone(),
        amount_cents: intent.amount_cents,
        currency: intent.currency.clone(),
        recipient: intent.recipient.clone(),
        purpose: intent.purpose.clone(),
        idempotency_key: intent.idempotency_key.clone(),
        status: intent.status.clone(),
        created_at: intent.created_at.clone(),
        approved_at: intent.approved_at.clone(),
    }
}

fn lock_intents() -> Result<std::fs::File> {
    let dir = crate::policy::root_dir()?.join("finance");
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(dir.join("intent.lock"))?;
    file.lock().context("could not lock payment intents")?;
    Ok(file)
}

fn intent_path() -> Result<PathBuf> {
    let dir = crate::policy::root_dir()?.join("finance");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("intents.json"))
}

fn read_intents() -> Result<IntentFile> {
    let path = crate::policy::root_dir()?
        .join("finance")
        .join("intents.json");
    if !path.exists() {
        return Ok(IntentFile::default());
    }
    let text = std::fs::read_to_string(&path)?;
    serde_json::from_str(&text).with_context(|| format!("could not read {}", path.display()))
}

fn write_intents(file: &IntentFile) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let body = serde_json::to_vec_pretty(file)?;
    let text = String::from_utf8(body.clone()).context("payment intent file is not UTF-8")?;
    if root_work::secrets::detect(&text).is_some() || digit_run(&text, 13) {
        anyhow::bail!("refusing a card or bank number");
    }
    let path = intent_path()?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body)?;
    let mut permissions = std::fs::metadata(&tmp)?.permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(&tmp, permissions)?;
    std::fs::rename(&tmp, &path).context("could not store payment intents")?;
    Ok(())
}

fn digit_run(text: &str, limit: usize) -> bool {
    let mut run = 0;
    for byte in text.bytes() {
        if byte.is_ascii_digit() {
            run += 1;
            if run >= limit {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

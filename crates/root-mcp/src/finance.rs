//! Read-only finance projection.
//!
//! The installed fixture files are the ledger. This module does not write a
//! second copy and it does not move money.

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

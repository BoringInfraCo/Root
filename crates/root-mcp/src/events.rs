//! Durable event ledger and explicit routes.
//!
//! Inbound connector events are recorded and nothing is invoked. A route may
//! create a delivery, and a delivery is only a record a harness can pull.
//! This module does not spawn an agent. Unrouted events stay recorded.

use crate::auth;
use crate::connector;
use crate::policy;
use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Ledger {
    events: Vec<Event>,
    routes: Vec<Route>,
    deliveries: Vec<Delivery>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Event {
    id: String,
    selector: String,
    source: String,
    idempotency_key: String,
    summary: String,
    correlation_id: String,
    status: String,
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Route {
    id: String,
    selector: String,
    workspace: String,
    harness: String,
    capabilities: Vec<String>,
    concurrency: u64,
    max_attempts: u64,
    approval_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Delivery {
    id: String,
    event_id: String,
    route_id: String,
    status: String,
    attempts: u64,
    approval_id: Option<String>,
    correlation_id: String,
}

#[derive(Debug, Serialize)]
pub struct EventView {
    pub id: String,
    pub selector: String,
    pub source: String,
    pub idempotency_key: String,
    pub summary: String,
    pub correlation_id: String,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct RouteView {
    pub id: String,
    pub selector: String,
    pub workspace: String,
    pub harness: String,
    pub capabilities: Vec<String>,
    pub concurrency: u64,
    pub max_attempts: u64,
    pub approval_required: bool,
}

#[derive(Debug, Serialize)]
pub struct DeliveryView {
    pub id: String,
    pub event_id: String,
    pub route_id: String,
    pub status: String,
    pub attempts: u64,
    pub approval_id: Option<String>,
    pub correlation_id: String,
}

#[derive(Debug, Serialize)]
pub struct IngestReport {
    pub connector_id: String,
    pub created: usize,
    pub duplicate: usize,
    pub deliveries: usize,
}

#[derive(Debug, Serialize)]
pub struct DeliverReport {
    pub handed: usize,
    pub skipped: usize,
    pub dead: usize,
}

pub fn record(source: &str, selector: &str, idempotency_key: &str, summary: &str) -> Result<()> {
    if idempotency_key.is_empty() || selector.is_empty() {
        anyhow::bail!("event selector and idempotency key are required");
    }
    let mut ledger = read()?;
    if ledger
        .events
        .iter()
        .any(|event| event.idempotency_key == idempotency_key)
    {
        return Ok(());
    }
    let correlation = auth::random_hex(8)?;
    let event = Event {
        id: format!("root_ev_{}", auth::random_hex(8)?),
        selector: selector.to_string(),
        source: source.to_string(),
        idempotency_key: idempotency_key.to_string(),
        summary: redact(summary),
        correlation_id: correlation,
        status: "recorded".to_string(),
        created_at: Utc::now().to_rfc3339(),
    };
    audit("event.record", &event.id, &event.correlation_id, "recorded")?;
    ledger.events.push(event);
    let index = ledger.events.len() - 1;
    let _ = open_deliveries(&mut ledger, index)?;
    write(&ledger)?;
    Ok(())
}

pub fn list_events() -> Result<Vec<EventView>> {
    Ok(read()?.events.iter().map(event_view).collect())
}

pub fn watch() -> Result<Vec<EventView>> {
    Ok(read()?
        .events
        .iter()
        .filter(|event| event.status != "acked")
        .map(event_view)
        .collect())
}

pub fn list_deliveries() -> Result<Vec<DeliveryView>> {
    Ok(read()?.deliveries.iter().map(delivery_view).collect())
}

pub fn list_routes() -> Result<Vec<RouteView>> {
    Ok(read()?.routes.iter().map(route_view).collect())
}

pub fn ack(id: &str) -> Result<EventView> {
    let mut ledger = read()?;
    let Some(event) = ledger.events.iter_mut().find(|event| event.id == id) else {
        anyhow::bail!("unknown event {id}");
    };
    event.status = "acked".to_string();
    let view = event_view(event);
    for delivery in ledger
        .deliveries
        .iter_mut()
        .filter(|item| item.event_id == id)
    {
        delivery.status = "acked".to_string();
    }
    write(&ledger)?;
    audit("event.ack", id, &view.correlation_id, "acked")?;
    Ok(view)
}

pub fn add_route(
    selector: &str,
    workspace: &str,
    harness: &str,
    capabilities: &[String],
    concurrency: u64,
    max_attempts: u64,
    approval_required: bool,
) -> Result<RouteView> {
    if selector.is_empty() || workspace.is_empty() || harness.is_empty() {
        anyhow::bail!("selector, workspace, and harness are required");
    }
    if concurrency == 0 || max_attempts == 0 {
        anyhow::bail!("concurrency and retries must be positive");
    }
    let mut ledger = read()?;
    let route = Route {
        id: format!("root_rt_{}", auth::random_hex(8)?),
        selector: selector.to_string(),
        workspace: workspace.to_string(),
        harness: harness.to_string(),
        capabilities: capabilities.to_vec(),
        concurrency,
        max_attempts,
        approval_required,
    };
    let view = route_view(&route);
    ledger.routes.push(route);
    let route_index = ledger.routes.len() - 1;
    backfill(&mut ledger, route_index)?;
    write(&ledger)?;
    audit("event.route", &view.id, "", "added")?;
    Ok(view)
}

pub fn ingest(connector_id: &str) -> Result<IngestReport> {
    let value = connector::exchange(connector_id, "ingest")?;
    let messages = value
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut ledger = read()?;
    let mut created = 0;
    let mut duplicate = 0;
    let mut deliveries = 0;
    for message in messages {
        let key = message
            .get("idempotency_key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if key.is_empty() {
            continue;
        }
        if ledger
            .events
            .iter()
            .any(|event| event.idempotency_key == key)
        {
            duplicate += 1;
            continue;
        }
        let selector = message
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("email.received")
            .to_string();
        let summary = redact(message.get("summary").and_then(Value::as_str).unwrap_or(""));
        let correlation = auth::random_hex(8)?;
        let event = Event {
            id: format!("root_ev_{}", auth::random_hex(8)?),
            selector,
            source: connector_id.to_string(),
            idempotency_key: key,
            summary,
            correlation_id: correlation.clone(),
            status: "recorded".to_string(),
            created_at: Utc::now().to_rfc3339(),
        };
        audit("event.ingest", &event.id, &correlation, "recorded")?;
        ledger.events.push(event);
        created += 1;
        let event_index = ledger.events.len() - 1;
        deliveries += open_deliveries(&mut ledger, event_index)?;
    }
    write(&ledger)?;
    Ok(IngestReport {
        connector_id: connector_id.to_string(),
        created,
        duplicate,
        deliveries,
    })
}

#[derive(Debug, Serialize)]
pub struct Wake {
    pub delivery_id: String,
    pub event_id: String,
    pub selector: String,
    pub idempotency_key: String,
    pub summary: String,
    pub workspace: String,
    pub harness: String,
    pub capabilities: Vec<String>,
    pub correlation_id: String,
}

/// Handed deliveries for one harness. Unrouted events are not included.
/// This does not start a process.
pub fn pull(harness: &str) -> Result<Vec<Wake>> {
    if harness.is_empty()
        || !harness
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        anyhow::bail!("harness must be a lowercase name");
    }
    let ledger = read()?;
    let mut wakes = Vec::new();
    for delivery in &ledger.deliveries {
        if delivery.status != "handed" {
            continue;
        }
        let Some(route) = ledger
            .routes
            .iter()
            .find(|route| route.id == delivery.route_id && route.harness == harness)
        else {
            continue;
        };
        let Some(event) = ledger
            .events
            .iter()
            .find(|event| event.id == delivery.event_id && event.status != "acked")
        else {
            continue;
        };
        wakes.push(Wake {
            delivery_id: delivery.id.clone(),
            event_id: event.id.clone(),
            selector: event.selector.clone(),
            idempotency_key: event.idempotency_key.clone(),
            summary: event.summary.clone(),
            workspace: route.workspace.clone(),
            harness: route.harness.clone(),
            capabilities: route.capabilities.clone(),
            correlation_id: delivery.correlation_id.clone(),
        });
    }
    Ok(wakes)
}

pub fn deliver() -> Result<DeliverReport> {
    let mut ledger = read()?;
    let mut handed = 0;
    let mut skipped = 0;
    let mut dead = 0;
    let routes = ledger.routes.clone();
    for delivery in ledger.deliveries.iter_mut() {
        if delivery.status == "acked" || delivery.status == "dead" || delivery.status == "queued" {
            skipped += 1;
            continue;
        }
        let Some(route) = routes.iter().find(|route| route.id == delivery.route_id) else {
            skipped += 1;
            continue;
        };
        if delivery.status == "awaiting_approval" {
            let Some(approval_id) = delivery.approval_id.as_deref() else {
                skipped += 1;
                continue;
            };
            if !connector::consume_approval(approval_id)? {
                skipped += 1;
                continue;
            }
            delivery.status = "pending".to_string();
        }
        if delivery.status != "pending" && delivery.status != "handed" {
            skipped += 1;
            continue;
        }
        if delivery.attempts >= route.max_attempts {
            delivery.status = "dead".to_string();
            dead += 1;
            audit(
                "event.deliver",
                &delivery.id,
                &delivery.correlation_id,
                "dead",
            )?;
            continue;
        }
        delivery.attempts += 1;
        delivery.status = "handed".to_string();
        handed += 1;
        audit(
            "event.deliver",
            &delivery.id,
            &delivery.correlation_id,
            "handed",
        )?;
    }
    write(&ledger)?;
    Ok(DeliverReport {
        handed,
        skipped,
        dead,
    })
}

fn open_deliveries(ledger: &mut Ledger, event_index: usize) -> Result<usize> {
    let event = ledger.events[event_index].clone();
    let mut created = 0;
    let routes: Vec<Route> = ledger
        .routes
        .iter()
        .filter(|route| route.selector == event.selector)
        .cloned()
        .collect();
    for route in routes {
        if ledger
            .deliveries
            .iter()
            .any(|item| item.event_id == event.id && item.route_id == route.id)
        {
            continue;
        }
        created += 1;
        push_delivery(ledger, &event, &route)?;
    }
    Ok(created)
}

fn backfill(ledger: &mut Ledger, route_index: usize) -> Result<()> {
    let route = ledger.routes[route_index].clone();
    let events: Vec<Event> = ledger
        .events
        .iter()
        .filter(|event| event.status != "acked" && event.selector == route.selector)
        .cloned()
        .collect();
    for event in events {
        if ledger
            .deliveries
            .iter()
            .any(|item| item.event_id == event.id && item.route_id == route.id)
        {
            continue;
        }
        push_delivery(ledger, &event, &route)?;
    }
    Ok(())
}

fn push_delivery(ledger: &mut Ledger, event: &Event, route: &Route) -> Result<()> {
    let active = ledger
        .deliveries
        .iter()
        .filter(|item| item.route_id == route.id && item.status != "acked" && item.status != "dead")
        .count() as u64;
    let (status, approval_id) = if active >= route.concurrency {
        ("queued".to_string(), None)
    } else if route.approval_required {
        let id = connector::queue_approval(&event.source, "event.wake", "write", &event.id)?;
        ("awaiting_approval".to_string(), Some(id))
    } else {
        ("pending".to_string(), None)
    };
    let correlation = auth::random_hex(8)?;
    audit("event.delivery", &event.id, &correlation, &status)?;
    ledger.deliveries.push(Delivery {
        id: format!("root_del_{}", auth::random_hex(8)?),
        event_id: event.id.clone(),
        route_id: route.id.clone(),
        status,
        attempts: 0,
        approval_id,
        correlation_id: correlation,
    });
    Ok(())
}

fn redact(summary: &str) -> String {
    if root_work::secrets::detect(summary).is_some() {
        "[redacted]".to_string()
    } else {
        summary.to_string()
    }
}

fn event_view(event: &Event) -> EventView {
    EventView {
        id: event.id.clone(),
        selector: event.selector.clone(),
        source: event.source.clone(),
        idempotency_key: event.idempotency_key.clone(),
        summary: event.summary.clone(),
        correlation_id: event.correlation_id.clone(),
        status: event.status.clone(),
        created_at: event.created_at.clone(),
    }
}

fn route_view(route: &Route) -> RouteView {
    RouteView {
        id: route.id.clone(),
        selector: route.selector.clone(),
        workspace: route.workspace.clone(),
        harness: route.harness.clone(),
        capabilities: route.capabilities.clone(),
        concurrency: route.concurrency,
        max_attempts: route.max_attempts,
        approval_required: route.approval_required,
    }
}

fn delivery_view(delivery: &Delivery) -> DeliveryView {
    DeliveryView {
        id: delivery.id.clone(),
        event_id: delivery.event_id.clone(),
        route_id: delivery.route_id.clone(),
        status: delivery.status.clone(),
        attempts: delivery.attempts,
        approval_id: delivery.approval_id.clone(),
        correlation_id: delivery.correlation_id.clone(),
    }
}

fn dir() -> Result<PathBuf> {
    let path = policy::root_dir()?.join("events");
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn ledger_path() -> Result<PathBuf> {
    Ok(dir()?.join("ledger.json"))
}

fn read() -> Result<Ledger> {
    let path = ledger_path()?;
    if !path.exists() {
        return Ok(Ledger::default());
    }
    let text = fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&text).unwrap_or_default())
}

fn write(ledger: &Ledger) -> Result<()> {
    let path = ledger_path()?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(ledger)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn audit(action: &str, entity: &str, correlation: &str, status: &str) -> Result<()> {
    let path = dir()?.join("audit.jsonl");
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::json!({
        "at": Utc::now().to_rfc3339(),
        "action": action,
        "entity": entity,
        "correlation_id": correlation,
        "status": status,
    });
    writeln!(file, "{line}")?;
    Ok(())
}

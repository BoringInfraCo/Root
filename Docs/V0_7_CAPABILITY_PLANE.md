# Root v0.7 — Open Capability Plane

**Status:** Proposed
**Theme:** *One local interface for every capability an agent may use.*

Root v0.6 makes engineering work resumable across supported harnesses. v0.7
should make capabilities portable across those harnesses without turning Root
into a closed hosted service. The durable asset is the user's capability plane;
the agent remains replaceable.

This direction is inspired by Bezalel's single agent-facing plane, event routing,
and personal capabilities, while keeping Root local-first, inspectable,
self-hostable, and provider-neutral.

## Product decision: one agent-facing interface

Root should have one stable agent-facing interface. The interface is the product
boundary, not necessarily one public cloud URL.

For v0.7:

- `rootd` owns state, policy, connector lifecycle, approvals, and events.
- Streamable HTTP MCP on loopback is the primary interface.
- `root mcp serve` remains as a stdio compatibility shim that connects to
  `rootd`; existing clients do not break.
- A Unix-domain socket is the default local transport. TCP requires an explicit
  opt-in and authentication.
- Every tool is namespaced (`work.*`, `email.*`, `computer.*`) and comes from a
  capability registry rather than being hard-coded into one giant server.

One interface matters because it gives every harness the same tool names,
authorization rules, audit history, events, and durable state. It also keeps
connector implementations replaceable.

## Architecture

```text
Codex / Claude / OpenCode / other MCP clients
                    |
           stdio shim or local MCP
                    |
                  rootd
       +------------+-------------+
       |            |             |
  capability     policy +      durable event
   registry      approvals        ledger
       |                          |
  connector hosts <--------- event router
       |
  Combie packages / built-in adapters / user connectors
```

Root owns the protocol, trust boundary, policy engine, audit ledger, and durable
state. Combie can own a growing catalog of connector implementations. Root must
not depend on a particular hosted Combie deployment: a connector package is
installed locally, declares its contract, and can be replaced by another package
with the same capability interface.

## Connector contract

Each connector ships a signed or content-addressed manifest containing:

- connector id, version, publisher, and executable digest;
- tools with JSON schemas and read/write/destructive risk classes;
- event types it may emit;
- required credential *names* and OAuth scopes, never values;
- network destinations and filesystem requirements;
- idempotency behavior and rate-limit metadata;
- health check, timeout, and resource limits.

Connectors run out of process with default-deny permissions. Installation,
credential binding, and dangerous actions are separate operations. Tool calls
and inbound events append to Root's audit ledger with redacted inputs and stable
correlation ids.

## Capability order

### 1. Connectors and events — v0.7 core

Build the registry, connector host protocol, policy model, audit ledger, and
event router first. Email, texting, money, and computer control should be
connectors on this substrate, not four unrelated subsystems.

Initial generic surfaces:

```text
root connector install|list|inspect|enable|disable|remove
root connector auth plan|bind|revoke
root capability list|inspect
root event list|watch|ack|route
root approval list|approve|deny
```

### 2. Email — first reference connector

Email exercises OAuth, search, pagination, attachments, inbound events, and
outbound side effects.

- Start with read/search/draft.
- Sending requires an approval by default.
- Inbound mail creates a durable event; it does not automatically invoke an
  agent until the user creates an event route.
- Provider packages can target Gmail, Microsoft Graph, IMAP/SMTP, or an email
  API without changing Root's tool contract.

### 3. Computer — second reference connector

Start with browser automation before full desktop control. It has a smaller and
more portable security boundary.

- Explicit per-session grant with visible target and expiry.
- Observe/screenshot is distinct from click/type.
- Downloads, credential entry, purchases, and destructive actions require
  elevated approval.
- Full desktop control remains experimental and OS-specific.

### 4. Texting — provider-neutral messaging

Do not make iMessage the protocol. Define `messages.threads`, `messages.read`,
`messages.draft`, and `messages.send`; implement SMS/Twilio or another portable
provider first. Platform-specific bridges can follow. Sending and unknown
recipients require approval.

### 5. Money — read-only before movement

Money is not a normal connector risk class.

- v0.7: transaction import, receipt ledger, categorization, and reconciliation.
- Later: create a payment intent with amount, recipient, purpose, and idempotency
  key.
- Root never stores card or bank credentials.
- Execution always requires an out-of-band human approval and provider-side
  limits. No autonomous transfer tool in v0.7.

## Automatic cross-machine sync

Today, a user manually moves three things:

1. Git carries repository content and `.root/agent.toml`.
2. A `.rootws` document carries durable work state.
3. `Rootfile` and `root.lock` carry the deterministic machine environment.

Automatic cross-machine sync means paired Root installations exchange the Root
state themselves. It does **not** mean syncing a working tree or copying secret
values.

The smallest honest v0.7 implementation is checkpoint sync:

- each installation has a device key and stable device id;
- a workspace has an end-to-end encrypted sync key;
- checkpoint creation appends immutable work events and environment/agent-intent
  references to an encrypted sync log;
- another paired device pulls the log, verifies hashes and signatures, and can
  restore/resume;
- credential names sync, values do not;
- Git still carries source code;
- conflicts are detected and surfaced; v0.7 does not claim live multi-writer
  collaboration.

The relay may be Root-hosted, user-hosted, or an object-store adapter. The relay
sees ciphertext, workspace ids, device ids, sizes, and timing only. A folder
transport remains available for air-gapped use.

Suggested commands:

```text
root device pair|list|revoke
root sync init|status|push|pull
root sync relay set|show
root checkpoint create --sync
```

## Event-driven agents

Inbound email, messages, webhooks, or scheduled events should enter one durable
event router. A route contains:

- event selector;
- target workspace and agent harness;
- allowed capabilities;
- concurrency and retry policy;
- whether human approval is required before invocation.

Events wake an agent only through an explicit route. Delivery is at-least-once,
so connectors and agent actions need idempotency keys. Every wake-up, retry,
approval, and resulting tool call is auditable.

## Proposed v0.7 milestones

### M1 — `rootd` and unified MCP

- local daemon, Unix socket, Streamable HTTP MCP, stdio shim;
- capability registry and namespaced tool discovery;
- bearer/session authentication for non-stdio transports;
- v0.6 MCP compatibility tests.

### M2 — Connector SDK and policy

- manifest/schema, process isolation, credential references, network grants;
- read/write/destructive risk classes and approval queue;
- Combie connector-package contract plus one example connector.

### M3 — Events plus email reference connector

- durable event ledger, subscriptions, retries, idempotency, explicit routes;
- email read/search/draft/send and inbound-message events;
- send approval and complete audit trail.

### M4 — Encrypted checkpoint sync

- device pairing, E2EE log, relay abstraction, push/pull/status;
- sync work state plus environment/agent-intent references;
- conflict detection, device revocation, offline recovery.

### M5 — Browser-computer connector

- observe and act grants, session expiry, approval boundaries;
- browser automation adapter and event/audit integration.

Texting and read-only finance follow once the connector and approval model has
survived the email and computer reference implementations.

## Explicit non-goals for v0.7

- no hosted secret vault requirement;
- no plaintext secrets in sync, logs, checkpoints, or connector manifests;
- no autonomous payments;
- no automatic execution for un-routed inbound events;
- no source-code sync or replacement for Git;
- no claim of conflict-free real-time multi-writer collaboration;
- no mandatory Root cloud account.

## Launch thesis

The open-source advantage is not cloning a competitor's list of integrations.
It is making the capability plane inspectable, self-hostable, provider-neutral,
and safe enough that users can trust it with personal capabilities. The v0.7
win condition is one excellent interface, one rigorous connector contract, one
event-driven reference connector, and encrypted checkpoint sync—not seven
half-integrated verticals.

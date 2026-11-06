# Edge Agent Harness Design

## 1. Positioning

Edge Agent Harness is a local-first, resource-bounded execution framework for AI agents on phones, tablets, and in-vehicle infotainment systems. The model proposes actions; the harness owns state, admission control, policy, scheduling, audit, recovery, and failure boundaries.

The framework is deliberately conservative:

1. Agent features are best-effort and must never block core platform functionality.
2. Model output is untrusted input and is never executed directly.
3. All actions pass through a deterministic controller.
4. Tasks receive hard resource and time budgets before they start.
5. Mobile and vehicle profiles share the same core but use different deployment topologies.

### Document map

- `docs/design.md` (this file): goals, architecture, state model, flows, and
  acceptance.
- `docs/roadmap.md`: milestone status and remaining work.
- `docs/HANDOFF.md`: current delivery and next-session starting points.
- `docs/build-paths.md`: how to build each platform target.
- `docs/adr/`: architecture decision records.

## 2. Goals and non-goals

### Goals

- Run on resource-constrained Android, iOS, Linux IVI, and Android Automotive systems.
- Provide a stable Rust core with native platform adapters.
- Enforce task budgets for duration, steps, tool calls, tokens, memory, and storage.
- Support structured tool calls with deterministic validation and policy checks.
- Support cancellation, pause, resume, crash recovery, and replayable audit logs.
- Adapt behavior to power, thermal, driving, background, and system-load state.
- Prioritize deterministic fast paths for high-frequency commands.
- Keep user data on the device unless cloud execution is explicitly enabled.
- Ensure agent failure does not compromise vehicle availability or safety boundaries.

### Non-goals

- Not an autonomous AGI runtime.
- Not a safety-certified vehicle component.
- Not a replacement for Android, iOS, or vehicle-platform permission systems.
- Not a guarantee that model-generated semantic answers are always correct.
- Not a first-version commitment to QNX, Windows, or unlimited background execution.
- Not a general model-training or fine-tuning system.

## 3. Supported platforms

### Tier 1

| Platform | Native layer | Notes |
|---|---|---|
| Android 10+ | Kotlin | App SDK, services, WorkManager, NNAPI/GPU/NPU adapters |
| iOS 16+ | Swift | SwiftUI, Core ML, Metal, lifecycle and thermal adapters |
| Linux IVI | Rust/systemd | Daemon deployment with cgroups and watchdogs |
| Android Automotive | Kotlin/AIDL | VHAL and Car Service access through a safety gateway |

### Tier 2

| Platform | Purpose |
|---|---|
| macOS | Development and iOS builds |
| Desktop Linux | Simulation, integration tests, and CI |
| Android Emulator | Device and automotive simulation |

### Tier 3

QNX, Windows, and HarmonyOS remain architectural possibilities. They are not initial delivery targets.

## 4. Language and process topology

The deterministic core is written in Rust:

```text
crates/
  harness-core       # implemented: task lifecycle, budgets, events, supervisor
  harness-model      # implemented: bounded model contracts and mock runtime
  harness-policy     # implemented: permission, risk, and mode checks
  harness-tools      # implemented: typed tool contracts and mock gateway
  harness-storage    # implemented: SQLite event/checkpoint persistence and replay
  harness-ipc        # implemented: bounded Unix-domain daemon IPC protocol
  harness-vehicle    # implemented: vehicle API whitelist and VHAL read gateway
  harness-profile    # implemented: device tiers and constrained budgets
  harness-privacy    # implemented: deterministic redaction pipeline
```

Platform directories:

```text
platform/
  linux-ivi/
    agent-daemon/       # Linux vehicle daemon binary
    vehicle-simulator/  # platform-state simulator for development
    deploy/             # systemd units
  android-automotive/
    vhal-reader/        # Kotlin VehiclePropertyManager adapter source
```

Phase 0 currently uses strongly typed tool actions rather than a JSON schema
parser. This gives the core a deterministic execution boundary before external
wire formats are introduced. A future JSON or MCP-like adapter must parse into
these typed actions before policy evaluation.

The bounded JSON adapter is now defined as:

```json
{ "type": "read_local", "key": "assistant.status" }
```

It rejects unknown fields, unknown action types, malformed payloads, empty
input, and payloads over 16 KiB. Parsing produces only a typed `ToolAction`; it
does not modify capability, approval, task class, or risk.

Platform layers are native:

- Android: Kotlin app/service, AIDL/Binder, JNI or UniFFI boundary.
- iOS: Swift app, SwiftUI, UniFFI or C ABI boundary.
- Linux IVI: Rust daemon under systemd with model/tool workers.
- Android Automotive: Kotlin gateway service plus Rust core.

### Deployment topologies

**Mobile in-process form**

```text
Kotlin/Swift UI
  -> Rust harness core
  -> platform model/tool adapters
```

This is the initial iOS topology. Android may use separate `:model` and `:tool` processes when available.

**Vehicle daemon form**

```text
Vehicle HMI
  -> Agent daemon
      -> Agent core
      -> Model worker
      -> Tool worker
      -> Vehicle API gateway
```

Model and tool workers are independently restartable and resource-capped.

The Linux Phase 2 skeleton uses a Unix-domain socket with newline-delimited
JSON. Each request is at most 16 KiB, rejects unknown fields, and deserializes
into a typed command enum. The daemon owns resource snapshots, admission policy,
task budgets, capabilities, and event retention. The IPC client can request task
lifecycle operations but cannot override policy or budgets.

Platform state arrives through a separate privileged channel: a bounded JSON
resource-snapshot file such as `/run/edge-agent/resource.json`. The daemon polls
the file and updates a shared snapshot store used by admission control. The
agent Unix socket never receives resource-snapshot updates, so an unprivileged
caller cannot forge a more permissive driving mode, thermal state, battery level,
or memory budget.

For development and integration tests, a `vehicle-simulator` binary writes a
cycled `ResourceSnapshot` to the resource file. It is not a vehicle control
component and never touches CAN, VHAL, sensors, or actuators.

The daemon notifies systemd with `READY=1` after startup and `WATCHDOG=1` at
half the configured watchdog interval. This detects a hung daemon, not just a
crashed one. Manual runs and tests without `NOTIFY_SOCKET` continue as no-ops.

The daemon accepts `--device-tier` (`flagship`, `mainstream`, or `entry`) and
constrains every IPC task budget through the corresponding device profile.

## 5. Core architecture

```text
┌───────────────────────────────────────────────┐
│ Platform Experience Layer                     │
│ Kotlin/Swift UI, voice, car HMI, approvals    │
└───────────────────────────────────────────────┘
                  │
┌───────────────────────────────────────────────┐
│ Agent Harness Core                            │
│ supervisor, state machine, scheduler, events  │
└───────────────────────────────────────────────┘
     │              │               │
┌──────────┐  ┌────────────┐  ┌──────────────────┐
│ Admission │  │ Policy and  │  │ Model Gateway    │
│ Control   │  │ Safety      │  │ structured output│
└──────────┘  └────────────┘  └──────────────────┘
     │              │               │
┌───────────────────────────────────────────────┐
│ Tool Gateway and Platform Runtime Layer       │
│ files, apps, vehicle APIs, model runtimes     │
└───────────────────────────────────────────────┘
```

## 6. User flows

### 6.0 Linux daemon control

1. The car HMI connects to the local Unix-domain socket.
2. It sends one newline-delimited JSON command.
3. The daemon parses the command into a typed enum and enforces the request-size
   limit.
4. The daemon applies its own admission policy, budget policy, and resource
   snapshot.
5. The supervisor commits any state change to SQLite before replying.
6. The daemon returns one bounded JSON response and closes the request.

### 6.1 Mobile request

1. User submits a request through text or voice.
2. The platform gateway resolves the interaction mode and required permissions.
3. Admission control assigns or rejects a task budget.
4. A deterministic intent fast path is attempted for known high-frequency commands.
5. If needed, the model gateway produces a structured action.
6. Policy validates schema, permissions, mode, risk, and budget.
7. A tool worker executes the approved action.
8. Observations update the task state and may trigger another bounded step.
9. The UI streams progress, waits for approval, or reports failure.

### 6.2 Vehicle request

1. Voice or HMI input is sent to the interaction gateway.
2. Driving mode is resolved before any model work.
3. The task class is checked against the current mode.
4. Long tasks are deferred until park or charging.
5. Allowed vehicle actions pass through the Vehicle API Gateway.
6. The vehicle service performs its own final safety validation.

### 6.3 Crash and resume

1. Every task change is persisted as an append-only event.
2. Checkpoints capture a compact recoverable state.
3. On restart, committed events are replayed.
4. Uncommitted writes are discarded.
5. Paused high-risk actions require fresh approval.
6. The user may cancel, resume, or discard the task.

## 7. State and data model

### 7.0 Vehicle API gateway

The agent reaches vehicle services only through a deterministic gateway:

```text
Vehicle API name
  -> manifest lookup
  -> capability check
  -> driving-mode check
  -> rate-limit check
  -> approval check
  -> platform vehicle adapter
```

The gateway owns the allowed-API whitelist, read/write classification, per-API
rate limits, and mode restrictions. It does not execute vehicle actions. A VHAL,
D-Bus, SOME/IP, or vendor adapter maps the allowed decision to the real service.

### Android Automotive read-only path

```text
agent -> VhalReadGateway::read(api_name, area_id, mode, capabilities)
  -> VehicleApiGateway decision
  -> VhalReadRequest { property_name, area_id }
  -> VehiclePropertyReader trait
  -> Kotlin VehiclePropertyManager / VHAL adapter
```

The read gateway never contains write APIs. A future Kotlin adapter maps stable
property names to real `VehiclePropertyIds` constants.

The Kotlin adapter source lives in:

```text
platform/android-automotive/vhal-reader/
```

It implements the Rust `VehiclePropertyReader` contract and uses
`VehiclePropertyManager` to read the mapped property. The mapping table is
explicit and must be adjusted to the target AAOS build; it is not policy.

### 7.1 Persistence and recovery model

Persistent storage uses SQLite as the local transactional boundary:

```sql
events(
  sequence INTEGER PRIMARY KEY,
  task_id TEXT NOT NULL,
  event_type INTEGER NOT NULL,
  payload TEXT NOT NULL,
  timestamp_ms INTEGER NOT NULL,
  checksum INTEGER NOT NULL
)

checkpoints(
  task_id TEXT PRIMARY KEY,
  task_json TEXT NOT NULL,
  sequence INTEGER NOT NULL,
  created_at_ms INTEGER NOT NULL,
  checksum INTEGER NOT NULL
)
```

Recovery rules:

1. Events are committed before state changes become recoverable.
2. Checkpoints are compact fast-forward snapshots, not the source of truth.
3. On startup, schema creation is transactional.
4. A checkpoint is accepted only when its sequence exists and its checksum
   matches.
5. Task state is rebuilt by replaying committed events after the checkpoint.
6. A committed event cannot be modified or deleted by the agent runtime.
7. Retention is bounded and rotates only after a checkpoint succeeds.
8. High-risk actions require a fresh approval after recovery.
9. Uncommitted writes are discarded.

Supervisor durability adds one more rule:

1. Before an in-memory task mutation is applied, the supervisor prepares and
   validates the event, commits it to the configured event sink, and only then
   mutates task state. The in-memory log is a bounded recent-event cache; SQLite
   is the durable source of truth.

This requires a write-ahead event boundary rather than "run first, log later":

```text
prepare event
  -> validate state, sequence, timestamp, checksum
  -> commit to SQLite event sink
  -> commit to bounded in-memory cache
  -> mutate task state
```

If the process stops before the final in-memory mutation, the committed event is
still authoritative and replay reconstructs the task. If the event commit fails,
the task is not mutated.

The first storage implementation stores core events and task checkpoints. It does
not trust serialized task state for safety decisions; status, budgets, and usage
are revalidated by Rust types after deserialization.

### 7.2 Checkpoint-anchored retention

Event retention is a maintenance operation, not a normal agent action:

```text
collect valid checkpoints
  -> verify checksum and task identity
  -> verify each checkpoint event still exists
  -> begin SQLite transaction
  -> temporarily remove the append-only delete trigger
  -> delete only events earlier than each checkpoint event
  -> restore append-only delete trigger
  -> commit transaction
```

The event referenced by a checkpoint is retained as an anchor. Replay starts
from the checkpoint and applies only later committed events. This keeps recovery
bounded without allowing a detached snapshot to become authoritative.

The daemon also enforces a global event-count ceiling through
`compact_if_over(max_events)`. Checkpoints are written for known tasks and
compaction runs only when the committed event count exceeds the threshold.

### 7.3 Device profiles and HMI approval

Budget selection is device-aware:

```text
DeviceTier::Flagship  -> 128 MiB task memory
DeviceTier::Mainstream -> 64 MiB task memory
DeviceTier::Entry     -> 32 MiB task memory
```

High-risk tool calls create bounded, expiry-aware approval requests:

```text
ApprovalStore::request(...)
  -> HMI renders the request
  -> HMI returns approve/reject
  -> ApprovalStore::decide(...)
```

Approval can expire and can be resolved only once. Model output cannot create or
resolve approval requests.

### Task

```text
TaskId
SessionId
Goal
Status: Pending | Planning | WaitingApproval | Running | WaitingModel
        | WaitingTool | Degraded | Blocked | Completed | Failed | Cancelled
TaskClass: UltraFast | FastQa | ReadOnly | Write | LongTask | VehicleAction
Budget
CreatedAt
UpdatedAt
```

### Budget

```text
DurationMs
MaxSteps
MaxToolCalls
MaxModelTokens
MaxStorageBytes
MaxMemoryBytes
Interruptible
```

Budgets are constrained by a `DeviceProfile`:

```text
Flagship
Mainstream
Entry
```

### Approval

```text
ApprovalId
TaskId
Tool
Reason
Status: Pending | Approved | Rejected | Expired
RequestedAt
ExpiresAt
```

### Planned extension: tool invocation and artifact records

The following are defined as planned extension records, not yet persisted:

### Tool invocation

```text
InvocationId
TaskId
ToolName
Arguments
RiskLevel: Low | Medium | High | Forbidden
Status: Pending | Denied | WaitingApproval | Running | Success | Failed | Timeout
StartedAt
EndedAt
RedactedSummary
```

### Event

```text
EventId
TaskId
Sequence
Type: TaskCreated | StateChanged | ModelStepConsumed | ToolCallConsumed
      | TaskFinished | TaskCancelled | TaskFailed
Payload
Timestamp
Checksum
```

### Artifact

```text
ArtifactId
TaskId
Kind: Text | Diff | Image | Table | Trace
StoragePath
Sha256
SizeBytes
CreatedAt
```

All event stores are bounded, redacted by default, and local-only by default.
Artifact persistence is planned.

## 8. Task state machine

```text
Pending
  -> Planning
Planning
  -> Running
  -> Blocked
  -> Failed
Running
  -> WaitingModel
  -> WaitingTool
  -> WaitingApproval
  -> Degraded
  -> Completed
  -> Failed
  -> Cancelled
WaitingModel
  -> Running
  -> Degraded
  -> Failed
  -> Cancelled
WaitingTool
  -> Running
  -> Failed
  -> Cancelled
WaitingApproval
  -> Running
  -> Blocked
  -> Cancelled
Degraded
  -> Running
  -> Blocked
  -> Completed
  -> Failed
  -> Cancelled
Running / WaitingModel / WaitingTool / WaitingApproval / Degraded
  -> Paused
Paused
  -> Running
  -> Blocked
  -> Cancelled
Blocked
  -> Running
  -> Cancelled
```

Only the supervisor performs transitions. Model output cannot mutate state directly.

## 9. Policy, permissions, and safety

Every proposed action follows this path:

```text
Structured action
  -> schema validation
  -> task permission check
  -> platform permission check
  -> mode check
  -> risk check
  -> resource check
  -> approval check
  -> deterministic executor
```

Risk defaults:

| Risk | Examples | Policy |
|---|---|---|
| Low | Read task-local state or bounded public data | Auto-allow |
| Medium | Write app-local files or change app settings | Confirmable |
| High | Delete data, send data off-device, alter settings | Explicit approval |
| Forbidden | Bypass sandbox, access vehicle control domain | Deny |

Vehicle constraints:

- Reverse gear disables the agent.
- High-speed driving limits tasks to ultra-fast or read-only voice interactions.
- Long tasks run only while parked or charging.
- Agent never accesses CAN, sensors, actuators, ADAS, chassis, powertrain, or brake domains.
- All vehicle access goes through a whitelisted, rate-limited, audited Vehicle API Gateway.
- The vehicle service remains the final safety authority.

Content from tools, files, web pages, and logs is data. It never inherits user authority.

## 10. Resource governance

Admission control checks:

- driving and interaction mode
- CPU headroom
- user attention availability
- battery and charging state
- thermal state
- available memory
- CPU headroom
- storage headroom
- current queue depth
- model availability
- task budget
- user presence and attention

Queues are bounded. New work is rejected or deferred instead of accumulating unbounded backpressure.

Vehicle service profile example:

```ini
[Service]
MemoryMax=250M
CPUWeight=20
IOWeight=20
Nice=10
Restart=on-failure
WatchdogSec=10
```

Exact values are device-specific. The presence of hard limits is mandatory.

## 11. Model gateway

The model layer has deterministic tiers:

1. Deterministic fast path for known commands.
2. Micro model/router for classification and extraction.
3. Local small model for constrained reasoning.
4. Optional cloud model with explicit user consent and privacy filtering.

Model calls receive:

- token budget
- first-token timeout
- total timeout
- cancellation token
- structured-output schema
- context budget

Invalid or non-conforming model output is an observation, not an executable action.

### 11.1 Cooperative bounded step loop

Every model or tool step passes through a supervisor-owned execution boundary:

```text
check cancellation
  -> check task deadline
  -> execute adapter with shared cancellation signal
  -> check cancellation again
  -> check completion timestamp against deadline
  -> commit durable terminal event if canceled or timed out
  -> otherwise account for model tokens or tool calls
```

Cancellation is cooperative. The supervisor cannot preempt arbitrary native
code, so model and tool adapters must poll the shared signal during generation,
I/O, or other long-running work. A result returned after cancellation is
discarded and is not charged to model or tool budgets.

### 11.2 Cloud fallback and redaction

Cloud execution is gated by `CloudFallbackPolicy`:

```text
enabled
  -> explicit user consent
  -> driving-mode restriction
  -> privacy-filter confirmation
  -> cloud path
```

Before any external transport or durable event, `RedactionPolicy` replaces
configured secrets and sensitive JSON keys with `[REDACTED]`.

A `ModelRouter` owns a local runtime and a cloud runtime. It selects the local
path whenever `CloudFallbackPolicy` rejects cloud and selects cloud only when
the policy permits it.

## 12. Context and memory

Context is assembled under an explicit token budget:

```text
current intent
task state
required tool observations
safety instructions
compact history
retrieved local knowledge
```

Large outputs are structured, summarized, truncated, or discarded. Sensitive values are redacted before storage or cloud transport. Long-term memory is opt-in, local by default, inspectable, exportable, and deletable.

## 13. Error handling and degradation

| Failure | Behavior |
|---|---|
| Model load failure | Use deterministic fast path or cached answer |
| Model timeout | Cancel call, retry once if budget allows, then degrade |
| Invalid structured output | Record error and request a corrected action if budget remains |
| Tool timeout | Cancel, mark failed, roll back uncommitted transaction |
| Permission denial | Enter Blocked and explain the missing grant |
| OOM or model crash | Restart worker; supervisor remains available |
| Thermal pressure | Use micro model, defer long tasks, or disable model work |
| Queue full | Reject or defer new tasks |
| Storage full | Stop write tasks and preserve audit events |
| Power loss | Discard uncommitted writes and replay committed events |
| Reverse gear | Pause or cancel nonessential vehicle tasks |

## 14. Testing strategy

- Unit tests for state transitions, budgets, policy, and tool validation.
- Integration tests with mock model and tool runtimes.
- Replay tests from event logs.
- Daemon-level restart tests that spawn the Linux binary, create a durable task,
  kill the process, restart it, and verify the task recovers from SQLite.
- Fault-injection tests for timeouts, crashes, disk errors, and cancellation.
- Resource soak tests on low-memory targets.
- Thermal and battery variation tests.
- Driving-mode policy tests.
- Prompt-injection corpus tests.
- Platform smoke tests for Android, iOS, Linux, and automotive simulation.

## 15. Acceptance checklist

- [ ] Rust core verified on Linux CI.
- [x] Task creation, admission, state transitions, cancellation, and failure work.
- [x] Structured action validation rejects invalid output.
- [x] Policy decisions are deterministic and auditable.
- [x] Resource budgets are enforced before and during execution.
- [x] Event log is append-only, bounded, and replayable.
- [x] Mock model and mock tools run integration scenarios.
- [x] Tool output cannot upgrade authority.
- [x] Reverse/high-speed vehicle modes restrict task classes.
- [x] Crash recovery discards uncommitted writes.
- [x] Device profiles constrain task budgets.
- [x] Bounded HMI approval contract works.
- [x] Consent-gated cloud fallback and redaction policies work.
- [x] Vehicle API whitelist, rate limits, and VHAL read gateway work.
- [x] Daemon restart recovers durable tasks.
- [x] Documentation and roadmap reflect the implemented scope.


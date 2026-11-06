# Handoff

## Date

2026-11-05

## Completed

- Created the Rust workspace with five Phase 0 crates:
  - `harness-core`
  - `harness-model`
  - `harness-policy`
  - `harness-tools`
- Implemented task status transitions and hard task budgets.
- Enforced wall-clock duration before any token or tool accounting is applied.
- Rejected model and tool accounting after a task reaches a terminal state.
- Implemented admission control for execution mode, queue depth, active tasks,
  thermal state, memory, storage, battery, model availability, and budget
  validity.
- Implemented deterministic capability and tool-risk policy checks.
- Implemented a bounded in-memory event log with sequence, timestamp, and
  checksum verification.
- Implemented typed model output and mock model runtime with token budget checks.
- Implemented typed local read/write tools, a manifest registry, and a mock tool
  gateway with policy and approval enforcement.
- Added an end-to-end high-speed vehicle mock scenario that creates a task,
  consumes a model step, performs a read-only tool call, completes the task, and
  verifies the event log.
- Derived active-task counts inside the supervisor so stale platform snapshots
  cannot bypass the concurrency limit.
- Added `harness-storage` with:
  - SQLite `events` and `checkpoints` tables.
  - Database triggers that reject event updates and deletes.
  - Event checksum and monotonic sequence validation.
  - Checkpoint checksum and referenced-event validation.
  - Task restoration from a checkpoint plus later committed events.
- Added SQLite reopen and supervisor replay/recovery scenarios.
- Added a write-ahead `EventSink` boundary in `harness-core`.
- Added `PersistentTaskSupervisor` in `harness-storage`.
- `TaskCreated` events now carry a serialized task snapshot for replay without an
  earlier checkpoint.
- Supervisor events are committed to SQLite before in-memory task mutation.
- Reopen restores all tasks from committed events and syncs the bounded event
  watermark.
- Added a failing-sink test proving event-store failure leaves task state
  unchanged.
- Added `CompactionReport` and transactional `compact_retention`.
- Checkpoint compaction removes only events before a valid checkpoint, retains
  the checkpoint anchor, and restores the append-only trigger.
- `PersistentTaskSupervisor::compact_retention` writes checkpoints for known
  tasks, compacts storage, and supports reopen after compaction.
- Added a bounded JSON adapter that strictly deserializes model output into typed
  `ToolAction` values and rejects unknown fields or action types.
- Added Ubuntu and macOS CI workflows for formatting, tests, and clippy.
- Added a bounded Unix-domain JSON IPC protocol and Linux daemon skeleton.
- Added a systemd unit with memory, CPU, I/O, priority, sandbox, and restart
  limits.
- Added `agent-daemon` CLI parsing for database, socket, mode, battery, charging,
  and event capacity.
- Added a daemon-level restart test that spawns the real binary, creates a
  durable task, kills the process, restarts it, and verifies SQLite recovery.
- Added a privileged resource-snapshot file watcher that updates admission
  inputs without exposing snapshot changes to the agent socket.
- Added an IPC-level test that a reverse snapshot blocks new tasks.
- Added a daemon-level test that writes a resource file and waits for the daemon
  to adopt it.
- Added a development vehicle simulator that writes cycled resource snapshots.
- Added `READY` / `WATCHDOG` systemd notification through `sd-notify`.
- The daemon reads `WATCHDOG_USEC`, halves it, and sends keep-alive notifications
  from a background thread; unit changes to `Type=notify` with `WatchdogSec=10`.
- Added a deterministic vehicle API gateway with whitelist, capability, mode,
  rate-limit, and approval checks.
- Added a Rust-side Android Automotive VHAL read-only gateway with a
  `VehiclePropertyReader` trait and mock reader.
- Added Kotlin adapter source that maps whitelisted property names to
  `VehiclePropertyManager` reads; compile against AAOS is still pending.
- Added an AAOS Gradle project skeleton with `android.car` library permission
  and vehicle-info manifest permissions.
- Added device profiles, bounded approval contract, consent-gated cloud fallback
  policy, and a deterministic redaction pipeline.
- Added a build-paths reference covering Rust, Linux daemon, AAOS, and planned
  Android/iOS mobile adapters.
- Added an HMI approval contract to the daemon IPC with request and resolve
  commands.
- Wired `DeviceProfile` into the daemon `--device-tier` flag so task budgets are
  constrained per device class.
- Polished `README.md` and `docs/design.md` to reflect the implemented modules,
  data model, acceptance checklist, project layout, and remaining boundaries.
- Added pause/resume lifecycle, bounded approval retention, CPU/attention
  admission inputs, a consent-gated model router, and event-count compaction.
- Added a bounded Unix-domain JSON IPC protocol and Linux daemon skeleton.
- Added a systemd unit with memory, CPU, I/O, priority, sandbox, and restart
  limits.
- Added cooperative `ExecutionSignal`, bounded model/tool step helpers, durable
  cancellation, and durable timeout/failure settlement.
- Late model/tool results are discarded after cancellation and are not charged to
  task budgets.
- Added persistent cancellation and timeout replay tests.

## Verification

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Current result: 69 tests pass. Tests were run on macOS/aarch64 with Rust 1.88.

## Important boundaries

- The current model output is strongly typed; JSON parsing is deliberately not in
  Phase 0.
- `TaskSupervisor` contains a bounded recent-event cache. Use
  `PersistentTaskSupervisor` when durable task state is required.
- Driving mode is represented at the policy layer; VHAL, Car Service, D-Bus, and
  platform mode adapters are not implemented.
- Model and tool runtimes are deterministic mocks.
- The project is not yet suitable for production vehicle deployment.

## Recommended next steps

1. Run the configured Linux CI workflow and update the roadmap with its result.
2. Introduce UniFFI/C ABI boundaries after the in-process API stabilizes.
3. Add Android and iOS platform adapters with thermal, battery, and background
   state inputs.

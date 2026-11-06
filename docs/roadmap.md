# Edge Agent Harness Roadmap

## Current status

**M1: core, persistence, Linux daemon, and AAOS scaffold complete**

The repository contains a documented, tested Rust core, durable SQLite
persistence, a Linux vehicle daemon, a vehicle API gateway, and an Android
Automotive source scaffold. It is not yet a mobile or vehicle product.

## Phase 0: architecture and core skeleton

**Status: complete except Linux CI verification**

- [x] Document goals, non-goals, platform support, and architecture.
- [x] Record foundational ADRs.
- [x] Create Rust workspace.
- [x] Implement task status, budget, and state-transition types.
- [x] Implement admission control.
- [x] Implement deterministic policy decision types.
- [x] Implement bounded event-log contract with integrity checks.
- [x] Add mock model/tool integration test.
- [x] Add a high-speed vehicle policy scenario.
- [x] Add SQLite-backed event storage.
- [x] Add task checkpoints.
- [x] Add event replay and crash-recovery tests.
- [x] Add a write-ahead durable event sink.
- [x] Add automatic `PersistentTaskSupervisor` reopen and restoration.
- [x] Add checkpoint-anchored event compaction.
- [x] Add a bounded JSON adapter for typed tool actions.
- [x] Add Linux and macOS CI workflows.
- [x] Add cooperative cancellation and timeout step boundaries.
- [ ] Verify Linux unit tests in an actual CI run.
- [x] Document Android, iOS, Linux IVI, and Android Automotive build paths.

Phase 0 notes:

- The current in-memory event log is bounded. SQLite persistence and checkpoint
  recovery are now being added before platform APIs are frozen.
- Model and tool runtimes are deterministic mocks suitable for architecture and
  policy tests. No production model backend is linked yet.
- Tests currently pass on the macOS development host; Linux target verification is
  still pending.
- Duration, token, step, tool-call, active-task, and execution-mode budgets have
  deterministic unit tests.
- Persistence starts in Phase 0 because replay and recovery are foundational to
  the platform APIs; they cannot be safely retrofitted later.
- SQLite storage now enforces append-only events with database triggers and
  validates event and checkpoint checksums before replay.
- `PersistentTaskSupervisor` now writes events to SQLite before mutating task
  state and restores all tasks from committed events on reopen.
- The in-memory event log is a bounded recent-event cache; committed SQLite
  events are the durable source of truth.
- Retention compaction is checkpoint-anchored and transactional. It removes only
  events before a validated checkpoint and restores the append-only trigger in
  the same transaction.
- JSON tool actions are bounded and strictly deserialized into the typed
  `ToolAction` enum before policy evaluation.
- Model and tool steps have supervisor-owned cancellation and deadline checks
  before and after adapter execution.
- The Linux daemon skeleton accepts bounded newline-JSON commands over a
  Unix-domain socket and replies with one bounded JSON response.
- A privileged `--resource-file` watcher updates the admission snapshot without
  exposing snapshot writes to the agent socket.
- The daemon sends `READY` and periodic `WATCHDOG` notifications through
  `sd-notify`; manual and test runs without `NOTIFY_SOCKET` are no-ops.
- CI is configured for Ubuntu and macOS; the Linux result is not verified until
  the workflow runs on a hosted runner.

## Phase 1: mobile SDK foundation

**Status: in progress**

- [ ] Expose core through UniFFI or C ABI.
- [ ] Add Kotlin Android package and lifecycle adapter.
- [ ] Add Swift iOS package and lifecycle adapter.
- [ ] Add platform thermal/power/resource signals.
- [ ] Add cancellation integration.
- [x] Add persistent checkpoint storage.
- [x] Add device profiles with constrained budgets.
- [x] Add bounded approval contract.
- [x] Add consent-gated cloud fallback policy.
- [x] Add deterministic redaction pipeline.

## Phase 2: Linux vehicle daemon

**Status: in progress**

- [x] Add a bounded Unix-domain JSON IPC crate.
- [x] Add a daemon executable and socket loop.
- [x] Add a systemd unit with resource limits.
- [x] Add service-manager watchdog notification.
- [ ] Add socket activation with a safe listener-from-fd boundary.
- [x] Add a privileged resource-snapshot file watcher.
- [ ] Add real thermal/battery/load/VHAL adapters that write the file.
- [ ] Separate model and tool workers.
- [x] Reuse SQLite crash/replay behavior through `PersistentTaskSupervisor`.
- [x] Add daemon-level crash/restart tests.
- [x] Add a development vehicle simulator that cycles platform modes.

## Phase 3: Android Automotive integration

**Status: in progress (AAOS compile pending)**

- [ ] Add AIDL service boundary.
- [x] Add a deterministic vehicle API gateway with whitelist and rate limits.
- [x] Add driving-mode and reverse-gear vehicle restrictions.
- [x] Add a Rust-side VHAL read-only gateway with a platform reader trait.
- [x] Add Kotlin adapter source implementing the reader.
- [x] Add an AAOS Gradle project skeleton with manifest permissions.
- [ ] Compile and verify the Kotlin adapter against an AAOS SDK.
- [x] Add HMI approval contract over the daemon IPC.
- [ ] Run fault-injection scenarios.

## Phase 4: reliability hardening

**Status: in progress**

- [x] Checkpoint-anchored event retention.
- [ ] Global event retention and rotation policy.
- [x] Redaction pipeline.
- [ ] Local vector/memory store with privacy controls.
- [x] Prompt-injection evaluation corpus.
- [x] Thermal and battery admission regression tests.
- [x] Resource and lint regression gates in CI.

## Phase 5: optional extensions

**Status: not started**

- [x] Consent-gated cloud fallback policy.
- [ ] Actual cloud runtime adapter.
- QNX port evaluation.
- Windows desktop support.
- HarmonyOS adapter evaluation.


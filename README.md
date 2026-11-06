# Edge Agent Harness

Edge Agent Harness is a local-first AI agent runtime for phones, tablets, Linux in-vehicle infotainment, and Android Automotive. It provides deterministic task supervision, admission control, policy enforcement, bounded execution, event logging, and recovery around untrusted model output.

## Current status

The deterministic core, durable persistence, Linux vehicle daemon, vehicle API
gateway, and Android Automotive source scaffold are implemented and tested.
This is not a production product yet; production model runtimes, real VHAL
integration, and mobile app packages are still pending.

## Architecture

```text
Platform Experience Layer
  Kotlin/Swift/HMI/voice
        |
Agent Harness Core
  supervisor, budgets, state machine, cancellation, approval
        |
Policy & Safety Layer
  capabilities, modes, vehicle whitelist, VHAL read gateway
        |
Runtime Layer
  SQLite, Unix IPC, model/tool adapters, redaction
```

Core principles:

1. The model proposes; the harness decides.
2. Every task has hard resource and time budgets.
3. All executable actions are structured and policy-checked.
4. Tool content never inherits user authority.
5. Agent failure must not affect platform or vehicle availability.

## Project layout

```text
crates/
  harness-core        # supervisor, budgets, state machine, events, approval
  harness-policy      # capabilities, modes, risk
  harness-tools       # typed tool actions + JSON adapter + mock gateway
  harness-model       # model contracts + mock runtime + cloud policy
  harness-storage     # SQLite events/checkpoints/replay
  harness-ipc         # Unix-domain JSON IPC
  harness-vehicle     # vehicle whitelist + VHAL read gateway
  harness-profile     # device tiers
  harness-privacy     # redaction
platform/
  linux-ivi/          # daemon, simulator, systemd units
  android-automotive/ # Kotlin VHAL reader + Gradle skeleton
docs/
  design.md
  roadmap.md
  HANDOFF.md
  build-paths.md
  adr/
```

## Implemented modules

- `harness-core`: task supervisor, lifecycle states, budgets, admission control, and bounded event log.
- `harness-storage`: SQLite event/checkpoint persistence and `PersistentTaskSupervisor` with write-ahead events, append-only triggers, checksums, compaction, reopen, and replay.
- `harness-policy`: execution modes, capabilities, tool risk, and deterministic decisions.
- `harness-model`: bounded model contracts and a deterministic mock runtime.
- `harness-tools`: typed tool actions, a bounded JSON adapter, a tool registry, and a mock gateway with policy enforcement.
- `harness-ipc`: bounded Unix-domain JSON IPC for the Linux vehicle daemon.
- `agent-daemon`: Linux vehicle daemon binary with CLI configuration and a systemd unit.
- `vehicle-simulator`: development tool that cycles driving/platform modes into the resource file.
- `harness-vehicle`: deterministic vehicle API gateway with whitelist, rate limits, and mode checks.
- `harness-profile`: device tiers and constrained budget selection.
- `harness-privacy`: deterministic redaction pipeline.
- `platform/android-automotive/vhal-reader`: Kotlin `VehiclePropertyManager` adapter source.
- `platform/android-automotive`: AAOS Gradle project skeleton with `android.car` manifest permissions.

## Not yet implemented

- Production model runtimes (llama.cpp / Core ML / NNAPI / NPU).
- Android phone and iOS mobile app packages.
- AAOS Gradle compilation against a real AAOS SDK.
- Real CarService / VHAL integration and OEM property mapping.
- Socket activation (blocked by the `unsafe_code = forbid` boundary).
- Model and tool worker process split.
- QNX / Windows / HarmonyOS ports.

## Reliability coverage

The test suite currently covers invalid state transitions, hard
duration/token/tool budgets, model and tool status guards, active-task
admission, high-speed driving policy, reverse-gear denial, thermal and battery
rejection, event checksums, durable event failure, checkpoint compaction,
typed JSON actions, cooperative cancellation, late-result discard, timeout
settlement, Unix-domain IPC, daemon restart recovery, resource-snapshot
adoption, watchdog interval selection, device-profile budgets, approval
expiry/resolution, cloud consent gating, redaction, prompt-injection corpus,
and vehicle API rate limiting.

## Documentation

- [Design](docs/design.md)
- [Roadmap](docs/roadmap.md)
- [Handoff](docs/HANDOFF.md)
- [Build paths](docs/build-paths.md)
- [ADR 0001: Rust core](docs/adr/0001-use-rust-for-agent-core.md)
- [ADR 0002: Best-effort agent service](docs/adr/0002-agent-is-best-effort.md)
- [ADR 0003: Admission control and budgets](docs/adr/0003-admission-control-and-hard-budgets.md)
- [ADR 0004: Untrusted model output](docs/adr/0004-model-output-is-untrusted.md)
- [ADR 0005: SQLite persistence](docs/adr/0005-sqlite-event-persistence.md)
- [ADR 0006: Write-ahead event sink](docs/adr/0006-write-ahead-event-sink.md)
- [ADR 0007: Checkpoint-compacted retention](docs/adr/0007-checkpoint-compacted-event-retention.md)
- [ADR 0008: Typed JSON action adapter](docs/adr/0008-typed-json-action-adapter.md)
- [ADR 0009: Cooperative cancellation and timeouts](docs/adr/0009-cooperative-cancellation-and-timeouts.md)
- [ADR 0010: Unix-domain JSON IPC](docs/adr/0010-unix-domain-json-ipc.md)
- [ADR 0011: Platform resource-snapshot file](docs/adr/0011-platform-resource-snapshot-file.md)
- [ADR 0012: systemd watchdog notification](docs/adr/0012-systemd-watchdog-notification.md)
- [ADR 0013: Vehicle API gateway](docs/adr/0013-vehicle-api-gateway.md)
- [ADR 0014: Android Automotive VHAL read gateway](docs/adr/0014-android-automotive-vhal-read-gateway.md)
- [ADR 0015: Device profiles and bounded approval](docs/adr/0015-device-profiles-and-bounded-approval.md)
- [ADR 0016: Consent-gated cloud fallback and redaction](docs/adr/0016-consent-gated-cloud-fallback-and-redaction.md)
- [ADR 0017: Pause/resume and approval retention](docs/adr/0017-pause-resume-and-bounded-approval-retention.md)
- [ADR 0018: CPU and attention admission](docs/adr/0018-cpu-and-attention-admission-inputs.md)
- [ADR 0019: Model router](docs/adr/0019-model-router-with-consent-gated-cloud-fallback.md)
- [ADR 0020: Event-count compaction threshold](docs/adr/0020-event-count-compaction-threshold.md)

## Planned platform support

| Tier | Platform |
|---|---|
| 1 | Android 10+, iOS 16+, Linux IVI, Android Automotive |
| 2 | macOS, desktop Linux, Android Emulator for development |
| 3 | QNX, Windows, HarmonyOS as future evaluations |

## Build and test

Requires Rust 1.88 or newer.

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

The current host is verified with 69 Rust unit/integration tests plus formatting and clippy checks. Ubuntu and macOS workflows are configured in `.github/workflows/ci.yml`; the Linux result is not verified until the workflow runs on a hosted runner.

## Linux daemon skeleton

```bash
cargo run -p agent-daemon -- \
  --db /var/lib/edge-agent/tasks.sqlite \
  --socket /run/edge-agent/agent.sock \
  --event-capacity 256 \
  --resource-file /run/edge-agent/resource.json \
  --device-tier mainstream \
  --max-events 4096
```

A systemd unit with resource limits is available at:

```text
platform/linux-ivi/deploy/edge-agent.service
```

For development, a simulator writes cycled platform states:

```bash
cargo run -p vehicle-simulator -- \
  --resource-file /run/edge-agent/resource.json \
  --poll-ms 1000
```


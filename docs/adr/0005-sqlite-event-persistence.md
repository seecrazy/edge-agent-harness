# ADR 0005: Use SQLite for event and checkpoint persistence

## Status

Accepted

## Context

The harness must survive process crashes, vehicle power loss, app termination, and daemon restarts on Android, iOS, and Linux. It needs an append-only event log for audit and replay, while checkpoints prevent replay cost from growing without bound.

## Decision

Use SQLite as the cross-platform persistence engine for agent events and task checkpoints. Commit events and checkpoints in transactions. Treat committed events as the source of truth and checkpoints as validated fast-forward snapshots. Reject checkpoints that reference missing events or have a mismatched checksum.

## Consequences

- The core gets crash-safe local persistence without a custom file format.
- SQLite adds a native dependency that must be validated on each platform.
- Retention and vacuuming must be explicit to prevent vehicle storage growth.
- High-risk approvals still require fresh confirmation after recovery.

# ADR 0017: Pause/resume lifecycle and bounded approval retention

## Status

Accepted

## Context

The task state machine does not support pause/resume, and the approval store
keeps resolved and expired requests forever. This makes long-running vehicle and
mobile sessions grow without bound and makes it impossible to suspend work when
driving mode changes or the user requests a pause.

## Decision

Add `TaskStatus::Paused` with legal transitions from `Running`,
`WaitingModel`, `WaitingTool`, `WaitingApproval`, and `Degraded`, and back to
`Running`.

Rework `ApprovalStore` to keep a bounded resolved queue. Resolving or expiring a
request frees its pending slot, and resolved requests are retained only up to a
configured maximum before being dropped.

## Consequences

- Tasks can be paused and resumed through the supervisor state machine.
- Pending approval capacity cannot be permanently consumed.
- Approval status lookup still works for a bounded recent window.

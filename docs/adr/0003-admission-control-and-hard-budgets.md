# ADR 0003: Require admission control and hard task budgets

## Status

Accepted

## Context

Resource-constrained devices cannot recover reliably after starting unbounded work. Token loops, tool loops, large logs, and concurrent model calls can exhaust memory, storage, battery, or thermal capacity.

## Decision

Before a task starts, admission control must grant a budget containing maximum duration, steps, tool calls, tokens, storage, memory, and queue depth. The supervisor enforces these limits during execution. If a budget is unavailable, the task is rejected or deferred.

## Consequences

- Resource exhaustion is prevented before work begins.
- Partial task progress is expected and must be recoverable.
- Complex jobs need explicit deferral to park, charging, or idle states.
- Budget accounting becomes part of every event trace.


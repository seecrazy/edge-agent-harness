# ADR 0020: Event-count compaction threshold

## Status

Accepted

## Context

Compaction currently removes events already covered by checkpoints, but there is
no global upper bound on the event table. On vehicle storage, the event table
must not grow without limit.

## Decision

Add `PersistentTaskSupervisor::compact_if_over(max_events, now_ms)`. The
supervisor writes checkpoints for known tasks and compacts only when the event
count exceeds the threshold. The daemon exposes `--max-events` with a default
limit.

## Consequences

- Event storage has a configurable ceiling.
- Compaction runs only when needed and never deletes checkpoint anchors.
- The maintenance operation stays inside the existing transactional compaction
  boundary.

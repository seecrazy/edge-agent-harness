# ADR 0006: Use a write-ahead event sink for supervisor mutations

## Status

Accepted

## Context

The first storage implementation could persist supervisor events, but callers had
to copy each in-memory event into SQLite manually. That creates two failure
windows: a mutation can exist without a durable event, or an event can be copied
after state has already changed. It also prevents the supervisor from reopening
with a durable event watermark.

## Decision

Introduce an `EventSink` boundary in the core supervisor. Every mutation first
prepares a checksummed event, validates it, commits it to the configured sink,
and then applies the in-memory task mutation. `TaskCreated` events carry the
serialized task so tasks can be reconstructed without an earlier checkpoint.
SQLite storage implements the sink and provides a persistent supervisor wrapper
that restores tasks and syncs the event watermark on startup.

## Consequences

- A committed SQLite event is authoritative even if the process stops before the
  in-memory mutation completes.
- An event write failure stops the mutation and leaves task state unchanged.
- The supervisor owns a bounded recent-event cache, not the durable log.
- `TaskCreated` events now contain full typed task snapshots and must remain
  redacted and bounded.

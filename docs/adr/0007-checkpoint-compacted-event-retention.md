# ADR 0007: Use checkpoint-anchored, transactional event compaction

## Status

Accepted

## Context

The SQLite event log is append-only and is the source of truth for recovery. On
phones and vehicles, an unbounded event history can eventually consume storage
and make startup replay unnecessarily expensive. At the same time, deleting
events without a validated checkpoint would make recovery depend on an
unanchored snapshot.

## Decision

Add a storage-maintenance operation that compacts only events already covered by
a valid checkpoint. For each task:

1. Validate the checkpoint checksum and task identity.
2. Require the checkpoint's referenced event to remain committed.
3. Delete only earlier events for that task.
4. Keep the checkpoint anchor event.
5. Perform all validation, trigger maintenance, deletion, and trigger restoration
   in one SQLite transaction.

The runtime cannot delete events. Compaction is exposed only as a storage
maintenance method. `PersistentTaskSupervisor` first writes checkpoints for its
known tasks and then invokes compaction under the same store lock.

## Consequences

- Replay can start from a checksum-validated checkpoint instead of the task's
  first event.
- A checkpoint remains anchored to a committed event; it is never an
  unanchored source of truth.
- The append-only runtime invariant is restored in the same transaction that
  performs maintenance deletion.
- Compaction does not yet remove completed-task checkpoints or enforce a global
  task-count limit; those require a separate retention policy.

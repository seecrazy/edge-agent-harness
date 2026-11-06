# ADR 0009: Treat cancellation and timeout as supervisor transitions

## Status

Accepted

## Context

Mobile and vehicle agents must be interruptible. Voice interruption, reverse
gear, low power, thermal pressure, user cancellation, and worker watchdogs can
all arrive while a model or tool is executing. A runtime cannot safely preempt
arbitrary native code, and a result returned after cancellation must not be
committed as if the user still wanted it.

## Decision

Make cancellation and timeout first-class supervisor operations:

1. Provide a cloneable cooperative `ExecutionSignal` backed by an atomic flag.
2. Model and tool adapters must poll the signal during long-running work.
3. Before each model/tool step, the supervisor checks cancellation and the task
   deadline.
4. After the step returns, the supervisor checks again before accounting for the
   result.
5. A canceled step is committed as one durable `TaskCancelled` event and its
   model/tool result is discarded.
6. A timeout is committed as one durable `TaskFailed` event with a bounded reason.
7. Only after the durable terminal event commits does the supervisor mutate the
   in-memory task state.

This is a cooperative boundary, not a promise of preemption. Real model and tool
adapters must still implement native cancellation, stop generation, close
sockets, or terminate workers as appropriate.

## Consequences

- Late results cannot be retroactively committed after user cancellation.
- Cancellation and timeout are replayable and auditable as terminal events.
- The core remains deterministic and testable with synchronous mock operations.
- Native runtime adapters must actively poll the signal; the supervisor cannot
  force-stop unsafe code inside a process.

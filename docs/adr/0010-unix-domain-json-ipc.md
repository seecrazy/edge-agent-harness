# ADR 0010: Use bounded Unix-domain JSON IPC for the Linux vehicle daemon

## Status

Accepted

## Context

The Linux vehicle daemon needs a local, restartable control boundary that a car
HMI, diagnostic shell, or future service manager can call without linking the
agent into the caller's process. Vehicle and mobile deployments also require a
strict request boundary before platform FFI is frozen.

## Decision

Use a Unix-domain socket with newline-delimited JSON requests and responses.
The initial protocol supports only supervisor operations:

- create a task using server-owned admission and budget policy
- get a task
- transition a task
- durably cancel a task

Requests are limited to 16 KiB, reject unknown fields, and deserialize into
typed command enums. The client cannot choose a budget, grant capability, change
policy, or delete events. Resource snapshots and task budgets are owned by the
daemon, not the caller. Each connection handles one bounded request and returns
one bounded JSON response.

The daemon is initially synchronous and handles one request per connection.
Concurrency, streaming progress, socket activation, and watchdog notification
remain later vehicle-daemon work.

## Consequences

- The car HMI can invoke the agent without an in-process dependency.
- The IPC boundary remains testable on macOS and Linux.
- A static server-owned resource snapshot is sufficient for the Phase 2 skeleton;
  real VHAL, thermal, battery, and load adapters must replace it before vehicle
  trials.
- Long-running model/tool progress is not streamed yet.
- A one-request-per-connection model is simple and reliable, but the daemon is
  not yet suitable for high-throughput HMI traffic.

# ADR 0011: Feed platform state through a privileged resource-snapshot file

## Status

Accepted

## Context

The vehicle daemon currently uses a static `ResourceSnapshot`. Driving mode,
thermal state, battery, queue depth, memory, and model availability change while
the daemon runs. The daemon must adopt those changes without letting an
unprivileged Unix-socket client forge a more permissive snapshot.

## Decision

Platform state is not exposed through the agent IPC protocol. Instead the daemon
optionally reads a bounded JSON resource-snapshot file owned by the platform
adapter, for example `/run/edge-agent/resource.json`. The daemon polls the file
and updates a shared read-write snapshot store used by admission control.

Socket clients cannot change the snapshot, budget, policy, or event retention.
The file path is set with `--resource-file`; the systemd unit grants write
access only to `/run/edge-agent`.

## Consequences

- Driving mode, thermal, battery, memory, storage, and model availability can
  change without restarting the daemon.
- The agent socket remains unprivileged with respect to platform state.
- Snapshot changes apply at the next admission decision rather than preemptively.
- The platform adapter is responsible for writing a valid, bounded file; the
  daemon ignores malformed updates.

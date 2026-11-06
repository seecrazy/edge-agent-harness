# ADR 0012: Notify systemd watchdog with a daemon-side keep-alive thread

## Status

Accepted

## Context

The vehicle daemon runs as a long-lived service. systemd must detect a hung
daemon even if the process has not exited. The current systemd unit restarts on
process failure but cannot detect a blocked accept loop or dead worker.

## Decision

Use the `sd-notify` crate to send `READY=1` after the daemon binds and
`WATCHDOG=1` on a keep-alive interval. The daemon reads the watchdog interval
from `WATCHDOG_USEC`, halves it, and notifies systemd from a background thread.
If the daemon stops sending notifications, systemd restarts the service.

The systemd unit changes to `Type=notify` and sets `WatchdogSec=10`. When
`NOTIFY_SOCKET` is absent (manual runs and tests), the notification calls are
no-ops and the daemon continues normally.

## Consequences

- systemd can restart a hung daemon, not only a crashed one.
- Manual daemon runs and tests do not require a service manager.
- Socket activation remains a separate, not-yet-implemented item because it
  requires a safe listener-from-fd boundary that does not yet exist.

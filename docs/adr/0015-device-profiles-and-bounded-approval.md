# ADR 0015: Device profiles and bounded approval contract

## Status

Accepted

## Context

Phones and vehicles have different memory, storage, thermal, and model
capabilities. A single static budget set is not safe across devices. Agent
actions that require user confirmation also need a bounded, deterministic
approval object so approval cannot be forged or replayed.

## Decision

Add a `harness-profile` crate with `DeviceTier` (Flagship, Mainstream, Entry),
`DeviceProfile`, and constrained budget selection. Add a bounded `ApprovalStore`
in `harness-core` with pending/approved/rejected/expired statuses and explicit
expiry timestamps.

Approval decisions are created and resolved only by the supervisor/platform
layer. Model output cannot create or approve an approval request.

## Consequences

- Budget selection is device-aware and testable.
- Approval requests are bounded, expiry-aware, and replay-resistant.
- Platform code can expose approval decisions through HMI without trusting model
  output.

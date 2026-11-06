# ADR 0018: CPU headroom and user attention in admission control

## Status

Accepted

## Context

The design promised CPU headroom and user-attention checks, but
`ResourceSnapshot` only carries memory, storage, thermal, battery, mode, and
model availability. A busy vehicle platform or a driver-focused interaction can
therefore bypass admission constraints.

## Decision

Add `cpu_headroom_percent` and `user_attention` to `ResourceSnapshot`.
Admission control rejects tasks when CPU headroom is below a floor and rejects
non-ultra-fast tasks when user attention is unavailable.

## Consequences

- CPU pressure prevents new agent work.
- Attention-sensitive tasks are deferred during driving or focus-heavy
  interactions.
- Every platform snapshot producer must supply the new fields.

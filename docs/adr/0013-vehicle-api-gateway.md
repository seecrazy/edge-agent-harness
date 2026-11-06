# ADR 0013: Deterministic vehicle API gateway with whitelist and rate limits

## Status

Accepted

## Context

The agent must not call arbitrary vehicle services. A model error, prompt
injection, or stale driving mode could otherwise invoke a write API, flood a
vehicle service, or act while reversing. The vehicle boundary must be
deterministic, testable, and independent of any VHAL implementation.

## Decision

Introduce a `harness-vehicle` crate with:

1. A manifest registry of allowed vehicle APIs with risk and capabilities.
2. A read-only/write classification.
3. Per-API rate limits with a sliding window.
4. Driving-mode and reverse-gear restrictions.
5. Approval requirements for write actions.

Every vehicle call must pass this gateway before reaching a VHAL, D-Bus,
SOME/IP, or vendor adapter. The gateway does not execute vehicle actions; it
returns a deterministic decision that the platform adapter then applies.

## Consequences

- Model output can never name an unregistered vehicle API.
- Reverse and high-speed modes are enforced before any adapter call.
- Write APIs require capability and approval and are rate-limited.
- A future Android Automotive VHAL adapter can map onto this gateway without
  changing core policy.

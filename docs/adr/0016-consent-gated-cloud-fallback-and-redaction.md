# ADR 0016: Consent-gated cloud fallback and redaction pipeline

## Status

Accepted

## Context

Cloud model calls are optional on mobile and vehicle devices. They must never
happen silently, and data leaving the device must be redacted before transport.
The current harness has a local-first model boundary but no explicit cloud gate.

## Decision

Add `CloudFallbackPolicy` in `harness-model` with:

- explicit enable flag
- required user consent
- driving-mode restrictions
- privacy-sensitive input rejection unless redaction is confirmed

Add a deterministic `RedactionPolicy` in `harness-privacy` that replaces
configured secrets and key-value pairs before any external transport or durable
event logging.

## Consequences

- Cloud execution is opt-in and cannot be triggered by model output.
- Privacy-sensitive values are redacted before leaving the device.
- The policy and redactor are deterministic and unit-tested without a network.

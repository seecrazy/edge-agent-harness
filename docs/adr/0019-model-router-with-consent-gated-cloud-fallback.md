# ADR 0019: Model router with consent-gated cloud fallback

## Status

Accepted

## Context

`CloudFallbackPolicy` only produces a decision. There is no runtime that selects
local vs cloud, so the policy cannot actually route model calls.

## Decision

Add `ModelRouter<L, C>` in `harness-model` that owns a local runtime, a cloud
runtime, and a `CloudFallbackPolicy`. A `RouterRequest` carries consent, mode,
privacy sensitivity, and redaction confirmation. The router selects local when
the policy rejects cloud and selects cloud only when the policy permits it.

## Consequences

- Cloud fallback becomes executable and testable.
- Local remains the default path whenever cloud is disabled or rejected.
- A production cloud adapter can be plugged in without changing policy.

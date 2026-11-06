# ADR 0004: Treat model output as untrusted structured input

## Status

Accepted

## Context

Model output can be incorrect, incomplete, malformed, or influenced by prompt injection through tool observations. Natural language must never directly trigger privileged operations.

## Decision

Require model actions to be structured data with a schema. Validate schema, permissions, risk, mode, and budget in deterministic code before execution. Treat content returned from tools, files, logs, or web pages as data with no inherited authority.

## Consequences

- Invalid output becomes a recoverable observation rather than an execution path.
- High-risk actions remain subject to approval regardless of prompt wording.
- Policy checks cannot be overridden by model instructions.
- The system needs explicit schemas for all executable tools.


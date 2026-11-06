# ADR 0008: Parse external model JSON only into typed actions

## Status

Accepted

## Context

Model runtimes and platform transports will deliver JSON, but executable
actions must not become a dynamic map of arbitrary tool names and arguments.
That would weaken schema validation, policy evaluation, replay determinism, and
prompt-injection resistance.

## Decision

Add a bounded JSON adapter at the `harness-tools` boundary. The adapter accepts
at most 16 KiB, rejects empty and malformed payloads, rejects unknown fields and
unknown action types, and deserializes only into the existing typed
`ToolAction` enum:

```json
{ "type": "read_local", "key": "assistant.status" }
{ "type": "write_local", "key": "theme", "value": "dark" }
```

Parsed actions still pass through the manifest registry and deterministic policy
engine. JSON parsing cannot grant capability, approval, risk downgrade, or task
class.

## Consequences

- External model output has a stable wire format before UniFFI/C ABI or MCP-like
  integration.
- Unsupported or extra fields are rejected rather than ignored.
- The adapter is bounded to prevent oversized observations from entering the
  parser.
- Adding a tool requires a typed variant, manifest, policy, and test rather than
  an open plugin argument map.

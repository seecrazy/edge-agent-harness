# ADR 0002: Treat the agent as a best-effort, non-safety-critical service

## Status

Accepted

## Context

Vehicle availability and driver safety take precedence over agent capability. A model failure, memory spike, or invalid action must never block navigation, media, climate, vehicle services, or safety systems.

## Decision

Run the vehicle agent as a low-priority, restartable, best-effort service. Do not place it in a safety loop. Do not grant direct access to CAN, sensors, actuators, ADAS, chassis, powertrain, or brake domains. Use a separate Vehicle API Gateway with whitelists, rate limits, driving-mode rules, timeouts, and audit logging.

## Consequences

- Agent failure has a bounded blast radius.
- The host vehicle service remains the final safety authority.
- Some capabilities are deferred or disabled while driving.
- The harness must be independently observable and restartable.


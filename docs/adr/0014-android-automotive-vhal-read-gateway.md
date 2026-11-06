# ADR 0014: Android Automotive VHAL read-only gateway

## Status

Accepted

## Context

Android Automotive exposes vehicle state through the Vehicle Hardware
Abstraction Layer (VHAL). The agent must read only whitelisted properties,
respect driving mode and rate limits, and never issue a write through a
read-only boundary. A Rust-side gateway must be testable without an Android
emulator or vendor VHAL implementation.

## Decision

Add `harness-vehicle` with:

1. A deterministic `VehicleApiGateway` containing whitelist, capability, mode,
   rate-limit, and approval checks.
2. A `VhalReadGateway` that maps approved read APIs to a
   `VhalReadRequest` (`property name + area id`).
3. A `VehiclePropertyReader` trait that a Kotlin Android Automotive adapter
   implements against `VehiclePropertyManager`.
4. A mock property reader for unit tests.

The read gateway rejects write APIs, unknown properties, reverse-mode reads,
missing capabilities, and rate-limited reads. Property names are stable symbols;
the future Kotlin adapter maps them to real `VehiclePropertyIds` constants.

## Consequences

- VHAL reads are testable without Android tooling.
- The gateway never writes vehicle state.
- The Kotlin adapter is a thin `VehiclePropertyReader` implementation, not a
  policy layer.
- Real Android property IDs are mapped in Kotlin, not embedded in Rust.

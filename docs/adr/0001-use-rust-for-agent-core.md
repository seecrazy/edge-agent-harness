# ADR 0001: Use Rust for the deterministic agent core

## Status

Accepted

## Context

The harness must run for long periods on phones and vehicles with constrained memory, thermal limits, and strict reliability requirements. Task state, policy evaluation, event storage, and scheduling are the highest-risk parts of the system.

## Decision

Implement the cross-platform harness core in Rust. Implement platform experience and system integration in Kotlin on Android/Android Automotive and Swift on iOS. Expose the Rust core through UniFFI, JNI, or a daemon IPC boundary.

## Consequences

- The core has memory safety without garbage-collection pauses.
- Policy and state-machine code can be strongly typed and exhaustively tested.
- Platform-specific behavior remains behind adapters.
- The team must maintain FFI and cross-compilation infrastructure.
- C++ remains acceptable only inside model runtimes or vendor middleware adapters.


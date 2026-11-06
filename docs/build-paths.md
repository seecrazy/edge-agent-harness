# Build paths

## Rust workspace (all crates)

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Verified on macOS/aarch64. CI also runs Ubuntu.

## Linux IVI daemon

```bash
cargo build -p agent-daemon
```

Binary:

```text
target/debug/agent-daemon
```

systemd unit:

```text
platform/linux-ivi/deploy/edge-agent.service
```

Development simulator:

```bash
cargo run -p vehicle-simulator -- --resource-file /run/edge-agent/resource.json
```

## Android Automotive

```bash
cd platform/android-automotive
./gradlew :vhal-reader:assembleDebug
```

Requires an AAOS build environment. The regular Android SDK does not expose
`android.car`.

## Android phone and iOS

The Rust core is platform-independent. Android phone and iOS adapters are not
built yet. Planned boundary:

- Android: Kotlin + UniFFI/JNI, NNAPI/GPU/NPU adapters.
- iOS: Swift + UniFFI/C ABI, Core ML/Metal adapters.

Cross-compile targets and Gradle/SwiftPM scaffolds will be added when those
platform adapters start.

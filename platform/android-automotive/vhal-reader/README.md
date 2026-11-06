# Android Automotive VHAL reader adapter

This module implements the Rust `VehiclePropertyReader` contract on Android
Automotive using `android.car.VehiclePropertyManager`.

## Contract

```kotlin
interface VehiclePropertyReader {
    fun read(request: VhalReadRequest): VhalReadResult
}
```

The Rust side already enforces:

- whitelist
- read-only classification
- driving-mode restrictions
- reverse-gear restrictions
- capability checks
- per-API rate limits

The Kotlin adapter is only a thin property read and value serialization layer.
It does not decide policy.

## Property mapping

| Rust API name | Android property | Notes |
|---|---|---|
| `vehicle.speed` | `VehiclePropertyIds.PERFORMANCE_VEHICLE_SPEED` | Verify against target AAOS |
| `vehicle.gear` | `VehiclePropertyIds.CURRENT_GEAR` | Verify against target AAOS |
| `vehicle.battery_level` | `VehiclePropertyIds.ENERGY_LEVEL` | Verify against target AAOS |
| `vehicle.charging_status` | `VehiclePropertyIds.CHARGING_STATUS` | Verify against target AAOS |
| `cabin.temperature` | target-specific | Must be mapped in the AAOS build |

The mapping table is build-specific. It is not policy and must be reviewed
against the exact AAOS property catalog before vehicle trials.

## Build note

`android.car` classes are available on Android Automotive platform builds, not
the regular Android SDK. This module is source that must be compiled inside an
AAOS build environment.

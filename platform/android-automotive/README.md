# Android Automotive Gradle project

This project builds the `vhal-reader` library against Android Automotive.

## Build

```bash
./gradlew :vhal-reader:assembleDebug
```

The `android.car` classes are only available inside an AAOS build environment.
A regular Android SDK cannot resolve them.

## Permissions

The manifest declares:

```xml
<uses-permission android:name="android.car.permission.CAR_VEHICLE_INFO" />
<uses-permission android:name="android.car.permission.CAR_VEHICLE_CONTROL" />
<uses-library android:name="android.car" android:required="true" />
```

Review the permission set against the target AAOS build. Read-only properties
generally need vehicle-info access; the Rust gateway still rejects writes.

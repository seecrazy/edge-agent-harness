//! Development vehicle platform state simulator.
//!
//! This binary writes a cycled `ResourceSnapshot` to the daemon's privileged
//! resource file so the vehicle daemon can be exercised without a real VHAL.
//! It is not a safety component and never touches vehicle control domains.

use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use harness_core::{ResourceSnapshot, ThermalState};
use harness_policy::ExecutionMode;

const CYCLE_SECONDS: &[(&str, u64)] = &[
    ("park", 10),
    ("drive_low_speed", 10),
    ("drive_high_speed", 10),
    ("reverse", 5),
    ("charging", 10),
    ("low_power", 5),
    ("system_busy", 5),
];

fn main() -> Result<(), Box<dyn Error>> {
    let mut resource_file = None;
    let mut poll_ms = 1_000;

    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--resource-file" => {
                resource_file = Some(PathBuf::from(
                    args.next().ok_or("--resource-file requires a value")?,
                ));
            }
            "--poll-ms" => {
                poll_ms = args
                    .next()
                    .ok_or("--poll-ms requires a value")?
                    .parse()
                    .map_err(|_| "--poll-ms must be a positive integer")?;
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    let resource_file = resource_file.ok_or("--resource-file is required")?;

    loop {
        let mode = mode_for_elapsed(seconds_since_epoch());
        let snapshot = resource_snapshot(mode);
        if let Err(error) = write_resource_file(&resource_file, snapshot) {
            eprintln!("vehicle-simulator: {error}");
        }
        thread::sleep(Duration::from_millis(poll_ms));
    }
}

fn seconds_since_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should advance")
        .as_secs()
}

fn mode_for_elapsed(elapsed_seconds: u64) -> ExecutionMode {
    let total: u64 = CYCLE_SECONDS.iter().map(|(_, seconds)| *seconds).sum();
    let mut cursor = elapsed_seconds % total;
    for (name, seconds) in CYCLE_SECONDS {
        if cursor < *seconds {
            return match *name {
                "park" => ExecutionMode::Park,
                "drive_low_speed" => ExecutionMode::DriveLowSpeed,
                "drive_high_speed" => ExecutionMode::DriveHighSpeed,
                "reverse" => ExecutionMode::Reverse,
                "charging" => ExecutionMode::Charging,
                "low_power" => ExecutionMode::LowPower,
                "system_busy" => ExecutionMode::SystemBusy,
                _ => ExecutionMode::Park,
            };
        }
        cursor -= *seconds;
    }
    ExecutionMode::Park
}

fn resource_snapshot(mode: ExecutionMode) -> ResourceSnapshot {
    ResourceSnapshot {
        mode,
        available_memory_bytes: 128 * 1024 * 1024,
        available_storage_bytes: 16 * 1024 * 1024,
        queue_depth: 0,
        active_tasks: 0,
        thermal_state: ThermalState::Nominal,
        battery_percent: 70,
        charging: matches!(mode, ExecutionMode::Charging),
        model_available: !matches!(mode, ExecutionMode::SystemBusy),
    }
}

fn write_resource_file(path: &PathBuf, snapshot: ResourceSnapshot) -> Result<(), Box<dyn Error>> {
    let content = serde_json::to_string(&snapshot)?;
    fs::write(path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_cycle_is_deterministic() {
        assert_eq!(mode_for_elapsed(0), ExecutionMode::Park);
        assert_eq!(mode_for_elapsed(9), ExecutionMode::Park);
        assert_eq!(mode_for_elapsed(10), ExecutionMode::DriveLowSpeed);
        assert_eq!(mode_for_elapsed(20), ExecutionMode::DriveHighSpeed);
        assert_eq!(mode_for_elapsed(30), ExecutionMode::Reverse);
        assert_eq!(mode_for_elapsed(50), ExecutionMode::SystemBusy);
        assert_eq!(mode_for_elapsed(60), ExecutionMode::Park);
    }

    #[test]
    fn snapshot_json_is_bounded_wire_format() {
        let snapshot = resource_snapshot(ExecutionMode::DriveHighSpeed);
        let json = serde_json::to_string(&snapshot).expect("snapshot should serialize");
        assert!(json.contains("\"mode\":\"drive_high_speed\""));
        assert!(json.contains("\"thermal_state\":\"nominal\""));
    }
}

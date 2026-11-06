//! Linux vehicle daemon skeleton.
//!
//! The daemon owns the SQLite database, admission policy, resource snapshot,
//! and Unix-domain socket. It accepts one bounded JSON request per connection.

use std::env;
use std::error::Error;
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use harness_core::{AdmissionController, ResourceSnapshot, ThermalState};
use harness_ipc::{IpcError, UnixIpcServer};
use harness_policy::ExecutionMode;
use harness_profile::{DeviceProfile, DeviceTier};
use sd_notify::{notify, NotifyState};

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = Arguments::from_env()?;
    if arguments.socket_path.exists() {
        let metadata = fs::metadata(&arguments.socket_path)?;
        if metadata.file_type().is_socket() {
            fs::remove_file(&arguments.socket_path)?;
        } else {
            return Err(format!(
                "socket path exists and is not a socket: {}",
                arguments.socket_path.display()
            )
            .into());
        }
    }

    let snapshot = arguments.resource_snapshot();
    let max_events = arguments.max_events;
    let mut server = UnixIpcServer::bind_with_profile(
        &arguments.database_path,
        &arguments.socket_path,
        AdmissionController::conservative(),
        arguments.event_capacity,
        snapshot,
        arguments.device_profile(),
    )?;
    let _ = notify(&[NotifyState::Ready]);

    if let Some(interval) = sd_notify::watchdog_enabled() {
        let poll_interval = watchdog_poll_interval(interval);
        thread::spawn(move || loop {
            thread::sleep(poll_interval);
            let _ = notify(&[NotifyState::Watchdog]);
        });
    }

    if let Some(resource_file) = arguments.resource_file {
        let store = server.resource_store();
        thread::spawn(move || loop {
            match fs::metadata(&resource_file) {
                Ok(metadata) if metadata.len() <= 16 * 1024 => {}
                _ => {
                    thread::sleep(Duration::from_secs(1));
                    continue;
                }
            }
            if let Ok(content) = fs::read_to_string(&resource_file) {
                if let Ok(updated) = parse_resource_snapshot(&content) {
                    if let Ok(mut guard) = store.write() {
                        *guard = updated;
                    }
                }
            }
            thread::sleep(Duration::from_secs(1));
        });
    }

    loop {
        match server.serve_once() {
            Ok(_) => {
                if let Err(error) = server.compact_if_over(max_events, now_ms()) {
                    eprintln!("agent-daemon: compaction failed: {error}");
                }
            }
            Err(IpcError::Io(error)) if error.kind() == std::io::ErrorKind::BrokenPipe => {}
            Err(error) => eprintln!("agent-daemon: {error}"),
        }
    }
}

/// Parsed daemon command-line arguments.
#[derive(Debug)]
struct Arguments {
    database_path: PathBuf,
    socket_path: PathBuf,
    event_capacity: usize,
    mode: ExecutionMode,
    charging: bool,
    battery_percent: u8,
    resource_file: Option<PathBuf>,
    device_tier: DeviceTier,
    max_events: u64,
}

impl Arguments {
    fn from_env() -> Result<Self, Box<dyn Error>> {
        let mut database_path = None;
        let mut socket_path = None;
        let mut event_capacity = 32;
        let mut mode = ExecutionMode::Park;
        let mut charging = false;
        let mut battery_percent = 80;
        let mut resource_file = None;
        let mut device_tier = DeviceTier::Mainstream;
        let mut max_events = 4_096;

        let mut args = env::args().skip(1);
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--db" => {
                    database_path =
                        Some(PathBuf::from(args.next().ok_or("--db requires a value")?));
                }
                "--socket" => {
                    socket_path = Some(PathBuf::from(
                        args.next().ok_or("--socket requires a value")?,
                    ));
                }
                "--event-capacity" => {
                    event_capacity = args
                        .next()
                        .ok_or("--event-capacity requires a value")?
                        .parse()
                        .map_err(|_| "--event-capacity must be a positive integer")?;
                }
                "--mode" => {
                    let value = args.next().ok_or("--mode requires a value")?;
                    mode = parse_mode(&value)?;
                }
                "--charging" => charging = true,
                "--battery" => {
                    battery_percent = args
                        .next()
                        .ok_or("--battery requires a value")?
                        .parse()
                        .map_err(|_| "--battery must be 0..=100")?;
                }
                "--resource-file" => {
                    resource_file = Some(PathBuf::from(
                        args.next().ok_or("--resource-file requires a value")?,
                    ));
                }
                "--device-tier" => {
                    let value = args.next().ok_or("--device-tier requires a value")?;
                    device_tier = parse_device_tier(&value)?;
                }
                "--max-events" => {
                    max_events = args
                        .next()
                        .ok_or("--max-events requires a value")?
                        .parse()
                        .map_err(|_| "--max-events must be a positive integer")?;
                }
                other => return Err(format!("unknown argument: {other}").into()),
            }
        }

        Ok(Self {
            database_path: database_path.ok_or("--db is required")?,
            socket_path: socket_path.ok_or("--socket is required")?,
            event_capacity,
            mode,
            charging,
            battery_percent,
            resource_file,
            device_tier,
            max_events,
        })
    }

    fn device_profile(&self) -> DeviceProfile {
        match self.device_tier {
            DeviceTier::Flagship => DeviceProfile::flagship(),
            DeviceTier::Mainstream => DeviceProfile::mainstream(),
            DeviceTier::Entry => DeviceProfile::entry(),
        }
    }

    fn resource_snapshot(&self) -> ResourceSnapshot {
        ResourceSnapshot {
            mode: self.mode,
            available_memory_bytes: 128 * 1024 * 1024,
            available_storage_bytes: 16 * 1024 * 1024,
            queue_depth: 0,
            active_tasks: 0,
            thermal_state: ThermalState::Nominal,
            battery_percent: self.battery_percent,
            charging: self.charging,
            model_available: true,
            cpu_headroom_percent: 80,
            user_attention: true,
        }
    }
}

fn parse_mode(value: &str) -> Result<ExecutionMode, String> {
    match value {
        "park" => Ok(ExecutionMode::Park),
        "drive_low_speed" => Ok(ExecutionMode::DriveLowSpeed),
        "drive_high_speed" => Ok(ExecutionMode::DriveHighSpeed),
        "reverse" => Ok(ExecutionMode::Reverse),
        "charging" => Ok(ExecutionMode::Charging),
        "low_power" => Ok(ExecutionMode::LowPower),
        "system_busy" => Ok(ExecutionMode::SystemBusy),
        _ => Err(format!("unknown mode: {value}")),
    }
}

fn parse_device_tier(value: &str) -> Result<DeviceTier, String> {
    match value {
        "flagship" => Ok(DeviceTier::Flagship),
        "mainstream" => Ok(DeviceTier::Mainstream),
        "entry" => Ok(DeviceTier::Entry),
        _ => Err(format!("unknown device tier: {value}")),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should advance")
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn parse_resource_snapshot(content: &str) -> Result<ResourceSnapshot, String> {
    if content.len() > 16 * 1024 {
        return Err("resource snapshot is too large".to_owned());
    }
    serde_json::from_str(content).map_err(|error| error.to_string())
}

fn watchdog_poll_interval(interval: Duration) -> Duration {
    interval
        .checked_div(2)
        .filter(|duration| !duration.is_zero())
        .unwrap_or(interval)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_drive_high_speed_mode() {
        assert_eq!(
            parse_mode("drive_high_speed"),
            Ok(ExecutionMode::DriveHighSpeed)
        );
        assert!(parse_mode("fly").is_err());
    }

    #[test]
    fn parses_resource_snapshot_file() {
        let snapshot = parse_resource_snapshot(
            r#"{
                "mode":"drive_high_speed",
                "available_memory_bytes":8388608,
                "available_storage_bytes":262144,
                "queue_depth":0,
                "active_tasks":0,
                "thermal_state":"nominal",
                "battery_percent":60,
                "charging":false,
                "model_available":true,
                "cpu_headroom_percent":80,
                "user_attention":true
            }"#,
        )
        .expect("snapshot should parse");

        assert_eq!(snapshot.mode, ExecutionMode::DriveHighSpeed);
        assert_eq!(snapshot.thermal_state, ThermalState::Nominal);
        assert_eq!(snapshot.battery_percent, 60);
    }

    #[test]
    fn parses_device_tier() {
        assert_eq!(parse_device_tier("entry"), Ok(DeviceTier::Entry));
        assert!(parse_device_tier("ultra").is_err());
    }

    #[test]
    fn watchdog_poll_interval_is_half() {
        assert_eq!(
            watchdog_poll_interval(Duration::from_secs(10)),
            Duration::from_secs(5)
        );
        assert_eq!(
            watchdog_poll_interval(Duration::from_millis(1)),
            Duration::from_micros(500)
        );
    }
}

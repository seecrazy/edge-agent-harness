//! Spawns the real Linux daemon binary and verifies SQLite recovery across
//! process restarts.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use harness_ipc::IpcResponse;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_dir() -> TempDir {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("edge-agent-daemon-{}-{id}", std::process::id()));
    fs::create_dir_all(&path).expect("temp dir should create");
    TempDir(path)
}

fn spawn_daemon(binary: &Path, working_dir: &Path, resource_file: Option<&Path>) -> Child {
    let mut command = Command::new(binary);
    command.current_dir(working_dir).args([
        "--db",
        "tasks.sqlite",
        "--socket",
        "agent.sock",
        "--event-capacity",
        "32",
    ]);
    if let Some(path) = resource_file {
        command.arg("--resource-file").arg(path);
    }
    command
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("daemon should spawn")
}

fn wait_for_socket(socket: &Path) {
    for _ in 0..100 {
        if UnixStream::connect(socket).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("daemon socket did not appear");
}

fn send_request(socket: &Path, request: &str) -> IpcResponse {
    let mut stream = UnixStream::connect(socket).expect("client should connect");
    stream
        .write_all(format!("{request}\n").as_bytes())
        .expect("request should write");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("response should read");
    serde_json::from_str(response.trim()).expect("response should parse")
}

#[test]
fn daemon_recovers_tasks_after_restart() {
    let temp = temp_dir();
    let _database = temp.0.join("tasks.sqlite");
    let socket = temp.0.join("agent.sock");
    let binary = Path::new(env!("CARGO_BIN_EXE_agent-daemon"));

    {
        let mut child = spawn_daemon(binary, &temp.0, None);
        wait_for_socket(&socket);
        let response = send_request(
            &socket,
            r#"{"type":"create_task","id":"restart-task","session_id":"s","goal":"durable","task_class":"fast_qa","needs_model":false,"now_ms":1000}"#,
        );
        let IpcResponse::Task { task } = response else {
            panic!("create should succeed");
        };
        assert_eq!(task.id, "restart-task");
        assert_eq!(task.status, harness_core::TaskStatus::Pending);

        child.kill().expect("daemon should stop");
        child.wait().expect("daemon should reap");
    }

    let mut child = spawn_daemon(binary, &temp.0, None);
    wait_for_socket(&socket);
    let response = send_request(&socket, r#"{"type":"get_task","task_id":"restart-task"}"#);
    let IpcResponse::Task { task } = response else {
        panic!("restored task should be returned");
    };
    assert_eq!(task.id, "restart-task");
    assert_eq!(task.status, harness_core::TaskStatus::Pending);
    assert_eq!(task.budget.max_duration_ms, 5_000);

    child.kill().expect("daemon should stop");
    child.wait().expect("daemon should reap");
}

#[test]
fn daemon_adopts_resource_snapshot_file() {
    let temp = temp_dir();
    let socket = temp.0.join("agent.sock");
    let resource_file = temp.0.join("resource.json");
    let binary = Path::new(env!("CARGO_BIN_EXE_agent-daemon"));

    let mut child = spawn_daemon(binary, &temp.0, Some(&resource_file));
    wait_for_socket(&socket);

    let create = send_request(
        &socket,
        r#"{"type":"create_task","id":"mode-task","session_id":"s","goal":"mode","task_class":"fast_qa","needs_model":false,"now_ms":1000}"#,
    );
    let IpcResponse::Task { task } = create else {
        panic!("create should succeed");
    };
    assert_eq!(task.id, "mode-task");

    let planning = send_request(
        &socket,
        r#"{"type":"transition","task_id":"mode-task","next":"planning","now_ms":1001}"#,
    );
    assert!(matches!(planning, IpcResponse::Task { .. }));
    let running = send_request(
        &socket,
        r#"{"type":"transition","task_id":"mode-task","next":"running","now_ms":1002}"#,
    );
    assert!(matches!(running, IpcResponse::Task { .. }));
    let completed = send_request(
        &socket,
        r#"{"type":"transition","task_id":"mode-task","next":"completed","now_ms":1003}"#,
    );
    assert!(matches!(completed, IpcResponse::Task { .. }));

    fs::write(
        &resource_file,
        r#"{
            "mode":"reverse",
            "available_memory_bytes":8388608,
            "available_storage_bytes":262144,
            "queue_depth":0,
            "active_tasks":0,
            "thermal_state":"nominal",
            "battery_percent":60,
            "charging":false,
            "model_available":true
        }"#,
    )
    .expect("resource file should write");

    let mut rejected = false;
    for attempt in 0..25 {
        let id = format!("reverse-task-{attempt}");
        let response = send_request(
            &socket,
            &format!(
                r#"{{"type":"create_task","id":"{id}","session_id":"s","goal":"mode","task_class":"fast_qa","needs_model":false,"now_ms":2000}}"#
            ),
        );
        if matches!(
            response,
            IpcResponse::Error {
                code: harness_ipc::IpcErrorCode::Rejected,
                ..
            }
        ) {
            rejected = true;
            break;
        }
        for (status, now) in [("planning", 2001), ("running", 2002), ("completed", 2003)] {
            let _ = send_request(
                &socket,
                &format!(
                    r#"{{"type":"transition","task_id":"{id}","next":"{status}","now_ms":{now}}}"#
                ),
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(rejected, "reverse snapshot should reject new tasks");

    child.kill().expect("daemon should stop");
    child.wait().expect("daemon should reap");
}

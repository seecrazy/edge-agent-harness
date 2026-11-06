//! Bounded Unix-domain JSON IPC for the Linux vehicle daemon.
//!
//! The protocol is deliberately small. Clients request supervisor operations;
//! they cannot supply task budgets, capabilities, policy decisions, or event
//! retention commands.

use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use harness_core::{
    AdmissionController, ApprovalStatus, ApprovalStore, CreateTaskRequest, ResourceSnapshot, Task,
    TaskBudget, TaskStatus,
};
use harness_policy::TaskClass;
use harness_profile::DeviceProfile;
use harness_storage::PersistentTaskSupervisor;
use serde::{Deserialize, Serialize};

/// Maximum request size accepted by the IPC protocol.
pub const MAX_REQUEST_BYTES: usize = 16 * 1024;

/// Error raised while parsing a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcRequestError {
    /// Request contained no non-whitespace data.
    Empty,
    /// Request exceeded the bounded protocol limit.
    TooLarge,
    /// Request was malformed or used an unsupported field or command.
    InvalidShape,
}

impl fmt::Display for IpcRequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "request is empty"),
            Self::TooLarge => write!(f, "request is too large"),
            Self::InvalidShape => write!(f, "request has an invalid shape"),
        }
    }
}

impl std::error::Error for IpcRequestError {}

/// Error returned by the IPC server.
#[derive(Debug)]
pub enum IpcError {
    /// Unix socket I/O failed.
    Io(std::io::Error),
    /// The request violated the bounded protocol.
    Request(IpcRequestError),
    /// The supervisor rejected the command.
    Internal(String),
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "ipc I/O error: {error}"),
            Self::Request(error) => write!(f, "ipc request error: {error}"),
            Self::Internal(message) => write!(f, "ipc internal error: {message}"),
        }
    }
}

impl std::error::Error for IpcError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Request(error) => Some(error),
            Self::Internal(_) => None,
        }
    }
}

impl From<std::io::Error> for IpcError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

/// Stable error code returned in an IPC response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcErrorCode {
    /// The request violated the wire contract.
    InvalidRequest,
    /// The supervisor rejected a command.
    Rejected,
    /// The daemon failed while handling a valid command.
    Internal,
}

/// Typed newline-JSON request accepted by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum IpcRequest {
    /// Create a task using daemon-owned admission and budget policy.
    CreateTask {
        /// Stable caller-owned task identifier.
        id: String,
        /// Session identifier.
        session_id: String,
        /// User-visible goal.
        goal: String,
        /// Task complexity class.
        task_class: TaskClass,
        /// Whether model work is required.
        needs_model: bool,
        /// Platform timestamp in milliseconds.
        now_ms: u64,
    },
    /// Return one task snapshot.
    GetTask {
        /// Task identifier.
        task_id: String,
    },
    /// Apply one legal supervisor transition.
    Transition {
        /// Task identifier.
        task_id: String,
        /// Next status.
        next: TaskStatus,
        /// Platform timestamp in milliseconds.
        now_ms: u64,
    },
    /// Durably cancel a task.
    Cancel {
        /// Task identifier.
        task_id: String,
        /// Bounded, already-redacted reason.
        reason: String,
        /// Platform timestamp in milliseconds.
        now_ms: u64,
    },
    /// Create one bounded HMI approval request.
    RequestApproval {
        /// Approval identifier.
        approval_id: String,
        /// Task requesting approval.
        task_id: String,
        /// Tool or action requiring approval.
        tool: String,
        /// Bounded, already-redacted reason.
        reason: String,
        /// Platform expiry timestamp.
        expires_at_ms: u64,
        /// Platform creation timestamp.
        now_ms: u64,
    },
    /// Resolve one HMI approval request.
    Approve {
        /// Approval identifier.
        approval_id: String,
        /// Whether the HMI approved the request.
        approved: bool,
        /// Platform decision timestamp.
        now_ms: u64,
    },
    /// Pause a task.
    Pause {
        /// Task identifier.
        task_id: String,
        /// Platform timestamp.
        now_ms: u64,
    },
    /// Resume a paused task.
    Resume {
        /// Task identifier.
        task_id: String,
        /// Platform timestamp.
        now_ms: u64,
    },
}

impl IpcRequest {
    /// Parses one bounded newline-JSON request.
    ///
    /// Unknown fields and unknown commands are rejected rather than ignored.
    pub fn parse(input: &str) -> Result<Self, IpcRequestError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(IpcRequestError::Empty);
        }
        if input.len() > MAX_REQUEST_BYTES {
            return Err(IpcRequestError::TooLarge);
        }
        serde_json::from_str(input).map_err(|_| IpcRequestError::InvalidShape)
    }
}

/// Typed newline-JSON response returned by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcResponse {
    /// Command returned one task snapshot.
    Task {
        /// Task state after the command.
        task: Task,
    },
    /// Command failed.
    Error {
        /// Stable error code.
        code: IpcErrorCode,
        /// Bounded diagnostic message.
        message: String,
    },
    /// Approval command returned a status.
    Approval {
        /// Current approval status.
        status: ApprovalStatus,
    },
}

impl IpcResponse {
    /// Creates a bounded error response.
    #[must_use]
    pub fn error(code: IpcErrorCode, message: impl fmt::Display) -> Self {
        Self::Error {
            code,
            message: message.to_string().chars().take(256).collect(),
        }
    }
}

/// Server-owned budget policy.
///
/// IPC clients cannot request a custom budget. The daemon derives a budget from
/// the typed task class before admission control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetPolicy {
    profile: DeviceProfile,
}

impl BudgetPolicy {
    /// Creates a policy using a device profile.
    #[must_use]
    pub const fn with_profile(profile: DeviceProfile) -> Self {
        Self { profile }
    }

    /// Returns the conservative budget for a task class.
    #[must_use]
    pub fn budget_for(self, task_class: TaskClass) -> TaskBudget {
        self.profile.budget_for(task_class)
    }
}

impl Default for BudgetPolicy {
    fn default() -> Self {
        Self {
            profile: DeviceProfile::mainstream(),
        }
    }
}

/// Synchronous Unix-domain supervisor server.
#[derive(Debug)]
pub struct UnixIpcServer {
    supervisor: PersistentTaskSupervisor,
    listener: UnixListener,
    resource_snapshot: Arc<RwLock<ResourceSnapshot>>,
    budget_policy: BudgetPolicy,
    max_request_bytes: usize,
    approval_store: ApprovalStore,
}

impl UnixIpcServer {
    /// Opens the persistent supervisor and binds the Unix-domain socket.
    pub fn bind(
        database_path: impl AsRef<Path>,
        socket_path: impl AsRef<Path>,
        admission: AdmissionController,
        event_capacity: usize,
        resource_snapshot: ResourceSnapshot,
    ) -> Result<Self, IpcError> {
        Self::bind_with_profile(
            database_path,
            socket_path,
            admission,
            event_capacity,
            resource_snapshot,
            DeviceProfile::mainstream(),
        )
    }

    /// Binds with an explicit device profile for budget constraints.
    pub fn bind_with_profile(
        database_path: impl AsRef<Path>,
        socket_path: impl AsRef<Path>,
        admission: AdmissionController,
        event_capacity: usize,
        resource_snapshot: ResourceSnapshot,
        profile: DeviceProfile,
    ) -> Result<Self, IpcError> {
        let supervisor = PersistentTaskSupervisor::open(database_path, admission, event_capacity)
            .map_err(|error| IpcError::Internal(error.to_string()))?;
        let listener = UnixListener::bind(socket_path)?;

        Ok(Self {
            supervisor,
            listener,
            resource_snapshot: Arc::new(RwLock::new(resource_snapshot)),
            budget_policy: BudgetPolicy::with_profile(profile),
            max_request_bytes: MAX_REQUEST_BYTES,
            approval_store: ApprovalStore::new(16),
        })
    }

    /// Returns the bound socket path where available.
    #[must_use]
    pub fn socket_path(&self) -> Option<PathBuf> {
        self.listener
            .local_addr()
            .ok()
            .and_then(|address| address.as_pathname().map(Path::to_path_buf))
    }

    /// Returns the shared platform resource-snapshot store.
    #[must_use]
    pub fn resource_store(&self) -> Arc<RwLock<ResourceSnapshot>> {
        Arc::clone(&self.resource_snapshot)
    }

    /// Replaces the platform resource snapshot used by admission control.
    ///
    /// This is a daemon-side privileged operation; it is not part of the agent
    /// IPC protocol.
    pub fn set_resource_snapshot(&self, snapshot: ResourceSnapshot) -> Result<(), IpcError> {
        let mut store = self
            .resource_snapshot
            .write()
            .map_err(|error| IpcError::Internal(error.to_string()))?;
        *store = snapshot;
        Ok(())
    }

    /// Accepts and handles one request.
    ///
    /// Each connection carries one bounded request and one response. Streaming
    /// progress and concurrent connections are intentionally deferred.
    pub fn serve_once(&mut self) -> Result<IpcResponse, IpcError> {
        let (stream, _) = self.listener.accept()?;
        let request = match read_request_line(&stream, self.max_request_bytes) {
            Ok(request) => request,
            Err(error) => {
                let response = IpcResponse::error(IpcErrorCode::InvalidRequest, error);
                write_response(&stream, &response)?;
                return Ok(response);
            }
        };

        let response = match IpcRequest::parse(&request) {
            Ok(request) => self.handle_request(request),
            Err(error) => IpcResponse::error(IpcErrorCode::InvalidRequest, error),
        };
        write_response(&stream, &response)?;
        Ok(response)
    }

    /// Compacts durable events when the committed count exceeds a threshold.
    pub fn compact_if_over(
        &mut self,
        max_events: u64,
        now_ms: u64,
    ) -> Result<harness_storage::CompactionReport, IpcError> {
        self.supervisor
            .compact_if_over(max_events, now_ms)
            .map_err(|error| IpcError::Internal(error.to_string()))
    }

    /// Handles one typed request without socket I/O.
    #[must_use]
    pub fn handle_request(&mut self, request: IpcRequest) -> IpcResponse {
        match request {
            IpcRequest::CreateTask {
                id,
                session_id,
                goal,
                task_class,
                needs_model,
                now_ms,
            } => {
                let budget = self.budget_policy.budget_for(task_class);
                let snapshot = match self.resource_snapshot.read() {
                    Ok(snapshot) => *snapshot,
                    Err(error) => {
                        return IpcResponse::error(
                            IpcErrorCode::Internal,
                            format!("resource snapshot lock poisoned: {error}"),
                        );
                    }
                };
                let request = CreateTaskRequest {
                    id: &id,
                    session_id: &session_id,
                    goal: &goal,
                    task_class,
                    budget: &budget,
                    needs_model,
                    snapshot: &snapshot,
                    now_ms,
                };
                self.result_response(|supervisor| supervisor.create_task(request))
            }
            IpcRequest::GetTask { task_id } => match self.supervisor.task(&task_id) {
                Some(task) => IpcResponse::Task { task },
                None => {
                    IpcResponse::error(IpcErrorCode::Rejected, format!("unknown task: {task_id}"))
                }
            },
            IpcRequest::Transition {
                task_id,
                next,
                now_ms,
            } => self.result_response(|supervisor| supervisor.transition(&task_id, next, now_ms)),
            IpcRequest::Cancel {
                task_id,
                reason,
                now_ms,
            } => {
                self.result_response(|supervisor| supervisor.cancel_task(&task_id, &reason, now_ms))
            }
            IpcRequest::RequestApproval {
                approval_id,
                task_id,
                tool,
                reason,
                expires_at_ms,
                now_ms,
            } => {
                if self.supervisor.task(&task_id).is_none() {
                    return IpcResponse::error(
                        IpcErrorCode::Rejected,
                        format!("unknown task: {task_id}"),
                    );
                }
                match self.approval_store.request(
                    &approval_id,
                    &task_id,
                    &tool,
                    &reason,
                    now_ms,
                    expires_at_ms,
                ) {
                    Ok(()) => IpcResponse::Approval {
                        status: ApprovalStatus::Pending,
                    },
                    Err(error) => IpcResponse::error(IpcErrorCode::Rejected, error),
                }
            }
            IpcRequest::Approve {
                approval_id,
                approved,
                now_ms,
            } => match self.approval_store.decide(&approval_id, approved, now_ms) {
                Ok(status) => IpcResponse::Approval { status },
                Err(error) => IpcResponse::error(IpcErrorCode::Rejected, error),
            },
            IpcRequest::Pause { task_id, now_ms } => {
                self.result_response(|supervisor| supervisor.pause_task(&task_id, now_ms))
            }
            IpcRequest::Resume { task_id, now_ms } => {
                self.result_response(|supervisor| supervisor.resume_task(&task_id, now_ms))
            }
        }
    }

    fn result_response<F>(&mut self, operation: F) -> IpcResponse
    where
        F: FnOnce(&mut PersistentTaskSupervisor) -> Result<Task, harness_storage::StorageError>,
    {
        match operation(&mut self.supervisor) {
            Ok(task) => IpcResponse::Task { task },
            Err(error) => IpcResponse::error(IpcErrorCode::Rejected, error),
        }
    }
}

fn read_request_line(stream: &UnixStream, max_bytes: usize) -> Result<String, IpcError> {
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();

    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Err(IpcError::Request(IpcRequestError::InvalidShape));
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take_len = newline.map_or(available.len(), |index| index + 1);
        if line.len() + take_len > max_bytes {
            return Err(IpcError::Request(IpcRequestError::TooLarge));
        }
        line.extend_from_slice(&available[..take_len]);
        reader.consume(take_len);

        if let Some(index) = newline {
            line.truncate(index);
            return String::from_utf8(line)
                .map_err(|_| IpcError::Request(IpcRequestError::InvalidShape));
        }
    }
}

fn write_response(mut stream: &UnixStream, response: &IpcResponse) -> Result<(), IpcError> {
    let mut output =
        serde_json::to_vec(response).map_err(|error| IpcError::Internal(error.to_string()))?;
    output.push(b'\n');
    stream.write_all(&output)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_core::ThermalState;
    use std::io::{Read, Write};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should advance")
            .as_nanos();
        std::env::temp_dir().join(format!("edge-agent-{prefix}-{unique}"))
    }

    fn snapshot() -> ResourceSnapshot {
        ResourceSnapshot {
            mode: harness_policy::ExecutionMode::Park,
            available_memory_bytes: TaskBudget::long_task().max_memory_bytes,
            available_storage_bytes: TaskBudget::long_task().max_storage_bytes,
            queue_depth: 0,
            active_tasks: 0,
            thermal_state: ThermalState::Nominal,
            battery_percent: 80,
            charging: false,
            model_available: true,
            cpu_headroom_percent: 80,
            user_attention: true,
        }
    }

    #[test]
    fn request_parser_rejects_dynamic_budget_and_unknown_fields() {
        assert_eq!(
            IpcRequest::parse(
                r#"{"type":"create_task","id":"task","session_id":"s","goal":"g",
                    "task_class":"fast_qa","needs_model":false,"now_ms":1,
                    "budget":{"max_duration_ms":999999}}"#
            ),
            Err(IpcRequestError::InvalidShape)
        );
        assert_eq!(IpcRequest::parse("   "), Err(IpcRequestError::Empty));
    }

    #[test]
    fn unix_ipc_creates_and_transitions_a_durable_task() {
        let database_path = temp_path("ipc-db");
        let socket_path = temp_path("ipc-socket");
        let mut server = UnixIpcServer::bind(
            &database_path,
            &socket_path,
            AdmissionController::conservative(),
            32,
            snapshot(),
        )
        .expect("server should bind");

        let server_thread = std::thread::spawn(move || {
            let first = server.serve_once().expect("first request");
            let second = server.serve_once().expect("second request");
            (first, second)
        });

        let mut client = UnixStream::connect(&socket_path).expect("client should connect");
        client
            .write_all(
                br#"{"type":"create_task","id":"ipc-task","session_id":"session","goal":"Read status","task_class":"fast_qa","needs_model":false,"now_ms":1000}
"#,
            )
            .expect("create request should write");
        let mut response = String::new();
        client
            .read_to_string(&mut response)
            .expect("create response should read");
        let response: IpcResponse =
            serde_json::from_str(response.trim()).expect("response should parse");
        let IpcResponse::Task { task } = response else {
            panic!("create should return a task");
        };
        assert_eq!(task.id, "ipc-task");
        assert_eq!(
            task.budget.max_duration_ms,
            TaskBudget::fast_qa().max_duration_ms
        );

        let mut client = UnixStream::connect(&socket_path).expect("client should reconnect");
        client
            .write_all(
                br#"{"type":"transition","task_id":"ipc-task","next":"planning","now_ms":1001}
"#,
            )
            .expect("transition request should write");
        let mut response = String::new();
        client
            .read_to_string(&mut response)
            .expect("transition response should read");
        let response: IpcResponse =
            serde_json::from_str(response.trim()).expect("response should parse");
        let IpcResponse::Task { task } = response else {
            panic!("transition should return a task");
        };
        assert_eq!(task.status, TaskStatus::Planning);

        let responses = server_thread.join().expect("server thread should finish");
        assert!(matches!(responses.0, IpcResponse::Task { .. }));
        assert!(matches!(responses.1, IpcResponse::Task { .. }));

        std::fs::remove_file(&socket_path).expect("socket should be removed");
        std::fs::remove_file(&database_path).expect("database should be removed");
    }

    #[test]
    fn resource_snapshot_update_blocks_tasks_in_reverse() {
        let database_path = temp_path("ipc-db-reverse");
        let socket_path = temp_path("ipc-socket-reverse");
        let server = UnixIpcServer::bind(
            &database_path,
            &socket_path,
            AdmissionController::conservative(),
            32,
            snapshot(),
        )
        .expect("server should bind");
        let mut reverse = snapshot();
        reverse.mode = harness_policy::ExecutionMode::Reverse;
        server
            .set_resource_snapshot(reverse)
            .expect("snapshot should update");

        let mut server = server;
        let response = server.handle_request(IpcRequest::CreateTask {
            id: "reverse-task".into(),
            session_id: "session".into(),
            goal: "should be rejected".into(),
            task_class: TaskClass::FastQa,
            needs_model: false,
            now_ms: 1_000,
        });

        assert!(matches!(
            response,
            IpcResponse::Error {
                code: IpcErrorCode::Rejected,
                ..
            }
        ));
        std::fs::remove_file(&socket_path).expect("socket should be removed");
        std::fs::remove_file(&database_path).expect("database should be removed");
    }

    #[test]
    fn hmi_approval_contract_requests_and_resolves() {
        let database_path = temp_path("ipc-db-approval");
        let socket_path = temp_path("ipc-socket-approval");
        let mut server = UnixIpcServer::bind(
            &database_path,
            &socket_path,
            AdmissionController::conservative(),
            32,
            snapshot(),
        )
        .expect("server should bind");

        let created = server.handle_request(IpcRequest::CreateTask {
            id: "approval-task".into(),
            session_id: "session".into(),
            goal: "needs approval".into(),
            task_class: TaskClass::Write,
            needs_model: false,
            now_ms: 1_000,
        });
        assert!(matches!(created, IpcResponse::Task { .. }));

        let requested = server.handle_request(IpcRequest::RequestApproval {
            approval_id: "approval-1".into(),
            task_id: "approval-task".into(),
            tool: "local.write".into(),
            reason: "write settings".into(),
            expires_at_ms: 2_000,
            now_ms: 1_000,
        });
        assert_eq!(
            requested,
            IpcResponse::Approval {
                status: ApprovalStatus::Pending
            }
        );

        let resolved = server.handle_request(IpcRequest::Approve {
            approval_id: "approval-1".into(),
            approved: true,
            now_ms: 1_500,
        });
        assert_eq!(
            resolved,
            IpcResponse::Approval {
                status: ApprovalStatus::Approved
            }
        );

        std::fs::remove_file(&socket_path).expect("socket should be removed");
        std::fs::remove_file(&database_path).expect("database should be removed");
    }

    #[test]
    fn ipc_can_pause_and_resume_tasks() {
        let database_path = temp_path("ipc-db-pause");
        let socket_path = temp_path("ipc-socket-pause");
        let mut server = UnixIpcServer::bind(
            &database_path,
            &socket_path,
            AdmissionController::conservative(),
            32,
            snapshot(),
        )
        .expect("server should bind");

        let created = server.handle_request(IpcRequest::CreateTask {
            id: "pause-task".into(),
            session_id: "session".into(),
            goal: "pause".into(),
            task_class: TaskClass::FastQa,
            needs_model: false,
            now_ms: 1_000,
        });
        assert!(matches!(created, IpcResponse::Task { .. }));
        let planning = server.handle_request(IpcRequest::Transition {
            task_id: "pause-task".into(),
            next: TaskStatus::Planning,
            now_ms: 1_001,
        });
        assert!(matches!(planning, IpcResponse::Task { .. }));
        let running = server.handle_request(IpcRequest::Transition {
            task_id: "pause-task".into(),
            next: TaskStatus::Running,
            now_ms: 1_002,
        });
        assert!(matches!(running, IpcResponse::Task { .. }));

        let paused = server.handle_request(IpcRequest::Pause {
            task_id: "pause-task".into(),
            now_ms: 1_003,
        });
        let IpcResponse::Task { task } = paused else {
            panic!("pause should return a task");
        };
        assert_eq!(task.status, TaskStatus::Paused);

        let resumed = server.handle_request(IpcRequest::Resume {
            task_id: "pause-task".into(),
            now_ms: 1_004,
        });
        let IpcResponse::Task { task } = resumed else {
            panic!("resume should return a task");
        };
        assert_eq!(task.status, TaskStatus::Running);

        std::fs::remove_file(&socket_path).expect("socket should be removed");
        std::fs::remove_file(&database_path).expect("database should be removed");
    }
}

//! Core task lifecycle, admission control, and append-only event primitives.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use harness_policy::{ExecutionMode, TaskClass};
use serde::{Deserialize, Serialize};

/// Error returned by harness core operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    /// Task identifier is already in use.
    DuplicateTask(String),
    /// Requested task was not found.
    UnknownTask(String),
    /// Requested state transition is invalid.
    InvalidTransition {
        /// Current task state.
        from: TaskStatus,
        /// Requested task state.
        to: TaskStatus,
    },
    /// Operation is not legal for the current task status.
    InvalidStatus {
        /// Current task status.
        current: TaskStatus,
        /// Operation attempted by the supervisor.
        operation: &'static str,
    },
    /// Task budget is malformed or exceeds platform limits.
    InvalidBudget(&'static str),
    /// Task execution would exceed its budget.
    BudgetExceeded(&'static str),
    /// The event log rejected an event.
    EventLog(EventLogError),
    /// A configured durable event sink rejected or failed an event.
    EventSink(EventSinkError),
    /// Persisted task data could not be serialized or deserialized.
    Serialization(String),
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateTask(id) => write!(f, "duplicate task: {id}"),
            Self::UnknownTask(id) => write!(f, "unknown task: {id}"),
            Self::InvalidTransition { from, to } => {
                write!(f, "invalid transition: {from:?} -> {to:?}")
            }
            Self::InvalidStatus { current, operation } => {
                write!(f, "invalid status for {operation}: {current:?}")
            }
            Self::InvalidBudget(reason) => write!(f, "invalid budget: {reason}"),
            Self::BudgetExceeded(reason) => write!(f, "budget exceeded: {reason}"),
            Self::EventLog(error) => write!(f, "event log error: {error}"),
            Self::EventSink(error) => write!(f, "event sink error: {error}"),
            Self::Serialization(error) => write!(f, "serialization error: {error}"),
        }
    }
}

impl std::error::Error for CoreError {}

/// Lifecycle status for an agent task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Task was created but has not started.
    Pending,
    /// Task is constructing a bounded plan.
    Planning,
    /// Task is waiting for a user or platform approval.
    WaitingApproval,
    /// Task is actively running.
    Running,
    /// Task is waiting for a model response.
    WaitingModel,
    /// Task is waiting for a tool response.
    WaitingTool,
    /// Task is using a fallback or reduced capability.
    Degraded,
    /// Task cannot continue without user or platform action.
    Blocked,
    /// Task completed successfully.
    Completed,
    /// Task failed.
    Failed,
    /// Task was cancelled.
    Cancelled,
}

impl TaskStatus {
    /// Stable wire name used in persisted state-change events.
    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Planning => "Planning",
            Self::WaitingApproval => "WaitingApproval",
            Self::Running => "Running",
            Self::WaitingModel => "WaitingModel",
            Self::WaitingTool => "WaitingTool",
            Self::Degraded => "Degraded",
            Self::Blocked => "Blocked",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }

    /// Reconstructs a status from its persisted wire name.
    #[must_use]
    pub fn parse_wire_name(value: &str) -> Option<Self> {
        match value {
            "Pending" => Some(Self::Pending),
            "Planning" => Some(Self::Planning),
            "WaitingApproval" => Some(Self::WaitingApproval),
            "Running" => Some(Self::Running),
            "WaitingModel" => Some(Self::WaitingModel),
            "WaitingTool" => Some(Self::WaitingTool),
            "Degraded" => Some(Self::Degraded),
            "Blocked" => Some(Self::Blocked),
            "Completed" => Some(Self::Completed),
            "Failed" => Some(Self::Failed),
            "Cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// Returns whether a supervisor-initiated transition is legal.
    #[must_use]
    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Pending, Self::Planning)
                | (Self::Pending, Self::Blocked)
                | (Self::Pending, Self::Cancelled)
                | (
                    Self::Planning,
                    Self::Running | Self::Blocked | Self::Failed | Self::Cancelled
                )
                | (
                    Self::Running,
                    Self::WaitingModel
                        | Self::WaitingTool
                        | Self::WaitingApproval
                        | Self::Degraded
                        | Self::Blocked
                        | Self::Completed
                        | Self::Failed
                        | Self::Cancelled
                )
                | (
                    Self::WaitingModel,
                    Self::Running | Self::Degraded | Self::Blocked | Self::Failed | Self::Cancelled
                )
                | (
                    Self::WaitingTool,
                    Self::Running | Self::Degraded | Self::Blocked | Self::Failed | Self::Cancelled
                )
                | (
                    Self::WaitingApproval,
                    Self::Running | Self::Blocked | Self::Failed | Self::Cancelled
                )
                | (
                    Self::Degraded,
                    Self::Running
                        | Self::Blocked
                        | Self::Completed
                        | Self::Failed
                        | Self::Cancelled
                )
                | (Self::Blocked, Self::Running | Self::Cancelled)
        )
    }
}

/// Hard resource limits attached to a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskBudget {
    /// Maximum task wall time.
    pub max_duration_ms: u64,
    /// Maximum supervisor steps.
    pub max_steps: u32,
    /// Maximum tool invocations.
    pub max_tool_calls: u32,
    /// Maximum model tokens.
    pub max_model_tokens: u64,
    /// Maximum task-local storage.
    pub max_storage_bytes: u64,
    /// Maximum memory reserved for this task.
    pub max_memory_bytes: u64,
    /// Whether this task can be interrupted.
    pub interruptible: bool,
}

impl TaskBudget {
    /// Creates a mobile-friendly ultra-fast budget.
    #[must_use]
    pub const fn ultra_fast() -> Self {
        Self {
            max_duration_ms: 1_500,
            max_steps: 2,
            max_tool_calls: 1,
            max_model_tokens: 256,
            max_storage_bytes: 64 * 1024,
            max_memory_bytes: 8 * 1024 * 1024,
            interruptible: true,
        }
    }

    /// Creates a vehicle-friendly fast QA budget.
    #[must_use]
    pub const fn fast_qa() -> Self {
        Self {
            max_duration_ms: 5_000,
            max_steps: 4,
            max_tool_calls: 2,
            max_model_tokens: 1_024,
            max_storage_bytes: 256 * 1024,
            max_memory_bytes: 32 * 1024 * 1024,
            interruptible: true,
        }
    }

    /// Creates a parked-only long-task budget.
    #[must_use]
    pub const fn long_task() -> Self {
        Self {
            max_duration_ms: 60_000,
            max_steps: 16,
            max_tool_calls: 12,
            max_model_tokens: 8_192,
            max_storage_bytes: 16 * 1024 * 1024,
            max_memory_bytes: 128 * 1024 * 1024,
            interruptible: true,
        }
    }

    /// Validates that a budget is positive and within platform hard limits.
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.max_duration_ms == 0 || self.max_steps == 0 || self.max_tool_calls == 0 {
            return Err(CoreError::InvalidBudget(
                "duration and counts must be positive",
            ));
        }
        if self.max_model_tokens == 0 || self.max_storage_bytes == 0 || self.max_memory_bytes == 0 {
            return Err(CoreError::InvalidBudget("resource limits must be positive"));
        }
        if self.max_steps > 64 || self.max_tool_calls > 64 {
            return Err(CoreError::InvalidBudget(
                "step and tool limits are too high",
            ));
        }
        if self.max_model_tokens > 32_768 {
            return Err(CoreError::InvalidBudget("model token limit is too high"));
        }
        if self.max_memory_bytes > 256 * 1024 * 1024 {
            return Err(CoreError::InvalidBudget("memory limit is too high"));
        }
        Ok(())
    }
}

/// Current consumption for a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TaskUsage {
    /// Elapsed task wall time.
    pub elapsed_ms: u64,
    /// Supervisor steps used.
    pub steps: u32,
    /// Tool calls used.
    pub tool_calls: u32,
    /// Model tokens used.
    pub model_tokens: u64,
    /// Task-local storage used.
    pub storage_bytes: u64,
}

/// A supervised task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// Stable task identifier.
    pub id: String,
    /// Session identifier.
    pub session_id: String,
    /// User-visible goal.
    pub goal: String,
    /// Complexity class.
    pub task_class: TaskClass,
    /// Current lifecycle state.
    pub status: TaskStatus,
    /// Granted budget.
    pub budget: TaskBudget,
    /// Current consumption.
    pub usage: TaskUsage,
    /// Creation timestamp supplied by the platform.
    pub created_at: u64,
    /// Last update timestamp supplied by the platform.
    pub updated_at: u64,
}

impl Task {
    /// Computes elapsed usage without mutating the task.
    fn elapsed_usage(&self, now_ms: u64) -> Result<u64, CoreError> {
        if now_ms < self.created_at {
            return Err(CoreError::InvalidStatus {
                current: self.status,
                operation: "clock regression",
            });
        }
        let elapsed_ms = now_ms - self.created_at;
        if elapsed_ms > self.budget.max_duration_ms {
            return Err(CoreError::BudgetExceeded("duration"));
        }
        Ok(elapsed_ms)
    }

    /// Validates a state transition without mutating the task.
    pub fn validate_transition(&self, next: TaskStatus, now_ms: u64) -> Result<(), CoreError> {
        if !self.status.can_transition_to(next) {
            return Err(CoreError::InvalidTransition {
                from: self.status,
                to: next,
            });
        }
        if now_ms < self.created_at {
            return Err(CoreError::InvalidStatus {
                current: self.status,
                operation: "clock regression",
            });
        }
        if !matches!(next, TaskStatus::Cancelled | TaskStatus::Failed) {
            self.elapsed_usage(now_ms)?;
        }
        Ok(())
    }

    /// Returns whether a model step may begin in the current status.
    #[must_use]
    pub fn can_execute_model_step(&self) -> bool {
        matches!(self.status, TaskStatus::Running | TaskStatus::WaitingModel)
    }

    /// Returns whether a tool step may begin in the current status.
    #[must_use]
    pub fn can_execute_tool_step(&self) -> bool {
        matches!(self.status, TaskStatus::Running | TaskStatus::WaitingTool)
    }

    /// Returns whether the task exceeded its wall-clock deadline.
    pub fn deadline_exceeded(&self, now_ms: u64) -> Result<bool, CoreError> {
        match self.elapsed_usage(now_ms) {
            Ok(_) => Ok(false),
            Err(CoreError::BudgetExceeded("duration")) => Ok(true),
            Err(error) => Err(error),
        }
    }

    /// Applies a legal supervisor transition.
    pub fn transition(&mut self, next: TaskStatus, now_ms: u64) -> Result<(), CoreError> {
        self.validate_transition(next, now_ms)?;
        if matches!(next, TaskStatus::Cancelled | TaskStatus::Failed) {
            self.status = next;
            self.updated_at = now_ms;
            return Ok(());
        }
        self.usage.elapsed_ms = self.elapsed_usage(now_ms)?;
        self.status = next;
        self.updated_at = now_ms;
        Ok(())
    }

    /// Records one model step and enforces model token budget.
    pub fn consume_model_step(&mut self, tokens: u64, now_ms: u64) -> Result<(), CoreError> {
        if !matches!(self.status, TaskStatus::Running | TaskStatus::WaitingModel) {
            return Err(CoreError::InvalidStatus {
                current: self.status,
                operation: "model step",
            });
        }
        self.usage.elapsed_ms = self.elapsed_usage(now_ms)?;
        if self.usage.steps >= self.budget.max_steps {
            return Err(CoreError::BudgetExceeded("steps"));
        }
        if self.usage.model_tokens.saturating_add(tokens) > self.budget.max_model_tokens {
            return Err(CoreError::BudgetExceeded("model tokens"));
        }
        self.usage.steps += 1;
        self.usage.model_tokens += tokens;
        Ok(())
    }

    /// Records one tool call and enforces the tool-call budget.
    pub fn consume_tool_call(&mut self, now_ms: u64) -> Result<(), CoreError> {
        if !matches!(self.status, TaskStatus::Running | TaskStatus::WaitingTool) {
            return Err(CoreError::InvalidStatus {
                current: self.status,
                operation: "tool call",
            });
        }
        self.usage.elapsed_ms = self.elapsed_usage(now_ms)?;
        if self.usage.tool_calls >= self.budget.max_tool_calls {
            return Err(CoreError::BudgetExceeded("tool calls"));
        }
        self.usage.tool_calls += 1;
        Ok(())
    }
}

/// Thermal state observed by the platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThermalState {
    /// Normal operation.
    Nominal,
    /// Reduced capability is advisable.
    Warm,
    /// Agent model work should stop.
    Hot,
}

/// Resource snapshot used by admission control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ResourceSnapshot {
    /// Current execution mode.
    pub mode: ExecutionMode,
    /// Available memory in bytes.
    pub available_memory_bytes: u64,
    /// Available task-local storage in bytes.
    pub available_storage_bytes: u64,
    /// Current bounded queue depth.
    pub queue_depth: u32,
    /// Current active task count.
    pub active_tasks: u32,
    /// Thermal state.
    pub thermal_state: ThermalState,
    /// Battery percentage from 0 to 100.
    pub battery_percent: u8,
    /// Whether the device is charging.
    pub charging: bool,
    /// Whether a local model runtime is available.
    pub model_available: bool,
}

/// Result of admission control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionDecision {
    /// Task may start with the granted budget.
    Admitted(TaskBudget),
    /// Task was rejected for the supplied reason.
    Rejected(&'static str),
}

/// Validated request used to create a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreateTaskRequest<'a> {
    /// Stable task identifier.
    pub id: &'a str,
    /// Owning session identifier.
    pub session_id: &'a str,
    /// User-visible goal.
    pub goal: &'a str,
    /// Complexity class.
    pub task_class: TaskClass,
    /// Requested budget.
    pub budget: &'a TaskBudget,
    /// Whether a model runtime is required.
    pub needs_model: bool,
    /// Current resource snapshot.
    pub snapshot: &'a ResourceSnapshot,
    /// Platform timestamp.
    pub now_ms: u64,
}

/// Bounded admission controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionController {
    /// Maximum concurrent tasks.
    pub max_active_tasks: u32,
    /// Maximum queued requests.
    pub max_queue_depth: u32,
    /// Maximum task duration.
    pub max_duration_ms: u64,
}

impl AdmissionController {
    /// Creates a conservative controller suitable for mobile and vehicle use.
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            max_active_tasks: 1,
            max_queue_depth: 8,
            max_duration_ms: 60_000,
        }
    }

    /// Evaluates whether a task may start now.
    pub fn evaluate(
        &self,
        task_class: TaskClass,
        requested: &TaskBudget,
        needs_model: bool,
        snapshot: &ResourceSnapshot,
    ) -> AdmissionDecision {
        if !snapshot.mode.allows(task_class) {
            return AdmissionDecision::Rejected("execution mode does not allow this task");
        }
        if snapshot.thermal_state == ThermalState::Hot {
            return AdmissionDecision::Rejected("thermal state is too high");
        }
        if snapshot.queue_depth >= self.max_queue_depth {
            return AdmissionDecision::Rejected("request queue is full");
        }
        if snapshot.active_tasks >= self.max_active_tasks {
            return AdmissionDecision::Rejected("active task limit reached");
        }
        if requested.max_duration_ms > self.max_duration_ms {
            return AdmissionDecision::Rejected("requested duration exceeds platform limit");
        }
        if requested.max_memory_bytes > snapshot.available_memory_bytes {
            return AdmissionDecision::Rejected("insufficient memory");
        }
        if requested.max_storage_bytes > snapshot.available_storage_bytes {
            return AdmissionDecision::Rejected("insufficient storage");
        }
        if !snapshot.charging && snapshot.battery_percent < 15 {
            return AdmissionDecision::Rejected("battery is too low");
        }
        if needs_model && !snapshot.model_available {
            return AdmissionDecision::Rejected("model runtime unavailable");
        }
        if requested.validate().is_err() {
            return AdmissionDecision::Rejected("invalid budget");
        }
        AdmissionDecision::Admitted(*requested)
    }
}

/// Types of events persisted by the harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventType {
    /// Task was created.
    TaskCreated,
    /// Task state changed.
    StateChanged,
    /// Model step was consumed.
    ModelStepConsumed,
    /// Tool call was consumed.
    ToolCallConsumed,
    /// Task reached a terminal state.
    TaskFinished,
    /// Task was canceled before or after a step result was produced.
    TaskCancelled,
    /// Task failed because of a timeout or adapter failure.
    TaskFailed,
}

/// Append-only event with a lightweight integrity checksum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// Monotonic sequence number.
    pub sequence: u64,
    /// Task identifier.
    pub task_id: String,
    /// Event type.
    pub event_type: EventType,
    /// Compact, already-redacted payload.
    pub payload: String,
    /// Platform timestamp in milliseconds.
    pub timestamp_ms: u64,
    /// FNV-1a checksum over all prior fields.
    pub checksum: u32,
}

impl EventType {
    /// Stable wire value used by the checksum.
    #[must_use]
    pub const fn checksum_value(self) -> u8 {
        match self {
            Self::TaskCreated => 1,
            Self::StateChanged => 2,
            Self::ModelStepConsumed => 3,
            Self::ToolCallConsumed => 4,
            Self::TaskFinished => 5,
            Self::TaskCancelled => 6,
            Self::TaskFailed => 7,
        }
    }

    /// Reconstructs an event type from its persisted wire value.
    #[must_use]
    pub const fn from_wire_value(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::TaskCreated),
            2 => Some(Self::StateChanged),
            3 => Some(Self::ModelStepConsumed),
            4 => Some(Self::ToolCallConsumed),
            5 => Some(Self::TaskFinished),
            6 => Some(Self::TaskCancelled),
            7 => Some(Self::TaskFailed),
            _ => None,
        }
    }
}

impl Event {
    /// Computes the event checksum.
    #[must_use]
    pub fn compute_checksum(
        sequence: u64,
        task_id: &str,
        event_type: EventType,
        payload: &str,
        timestamp_ms: u64,
    ) -> u32 {
        let mut hash: u32 = 2_166_136_261;
        for byte in sequence
            .to_le_bytes()
            .iter()
            .copied()
            .chain(task_id.bytes())
            .chain([event_type.checksum_value()])
            .chain(payload.bytes())
            .chain(timestamp_ms.to_le_bytes())
        {
            hash ^= u32::from(byte);
            hash = hash.wrapping_mul(16_777_619);
        }
        hash
    }
}

/// Bounds a terminal reason before it enters the durable event log.
fn bounded_reason(reason: &str) -> String {
    reason.chars().take(128).collect()
}

/// Error returned by the in-memory event log contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventLogError {
    /// Event sequence is not monotonic.
    NonMonotonicSequence,
    /// Event timestamp moved backwards.
    NonMonotonicTimestamp,
    /// Stored checksum did not match recomputed checksum.
    ChecksumMismatch,
}

impl fmt::Display for EventLogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonMonotonicSequence => write!(f, "event sequence is not monotonic"),
            Self::NonMonotonicTimestamp => write!(f, "event timestamp is not monotonic"),
            Self::ChecksumMismatch => write!(f, "event checksum mismatch"),
        }
    }
}

impl std::error::Error for EventLogError {}

/// Error returned by a durable event sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventSinkError {
    /// The external event store rejected an event or failed to commit it.
    External(String),
}

impl fmt::Display for EventSinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::External(error) => write!(f, "external event store error: {error}"),
        }
    }
}

impl std::error::Error for EventSinkError {}

/// Durable write-ahead sink for supervisor events.
pub trait EventSink: fmt::Debug + Send {
    /// Commits one already prepared and checksummed event.
    fn append_event(&mut self, event: &Event) -> Result<(), EventSinkError>;
}

/// Cooperative cancellation flag shared with model and tool adapters.
#[derive(Debug, Clone, Default)]
pub struct ExecutionSignal {
    cancelled: Arc<AtomicBool>,
}

impl ExecutionSignal {
    /// Creates an execution signal in the non-canceled state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cooperative cancellation.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Returns whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// Completion metadata returned by a synchronous bounded step adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepCompletion {
    /// Platform timestamp when the adapter completed.
    pub completed_at_ms: u64,
}

/// Completion metadata returned by a bounded model adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelStepCompletion {
    /// Tokens consumed by the completed model call.
    pub tokens: u64,
    /// Platform timestamp when the adapter completed.
    pub completed_at_ms: u64,
}

/// Final supervisor decision for a bounded model or tool step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOutcome<T> {
    /// The step completed and its usage was committed.
    Completed(T),
    /// Cancellation was observed and the adapter result was discarded.
    Cancelled,
    /// A timeout or adapter failure was durably recorded.
    Failed,
}

/// Approval lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    /// Waiting for a user or HMI decision.
    Pending,
    /// Approved.
    Approved,
    /// Rejected.
    Rejected,
    /// Expired before a decision was made.
    Expired,
}

/// Bounded approval request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    /// Stable approval identifier.
    pub id: String,
    /// Task identifier requesting approval.
    pub task_id: String,
    /// Tool or action requiring approval.
    pub tool: String,
    /// Bounded, already-redacted reason.
    pub reason: String,
    /// Current status.
    pub status: ApprovalStatus,
    /// Platform timestamp when the request was created.
    pub requested_at_ms: u64,
    /// Platform timestamp when the request expires.
    pub expires_at_ms: u64,
}

/// Error returned by the approval store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalError {
    /// Approval identifier does not exist.
    NotFound,
    /// Approval identifier already resolved.
    AlreadyResolved,
    /// Approval expired.
    Expired,
    /// Pending approval limit reached.
    LimitReached,
}

impl fmt::Display for ApprovalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "approval not found"),
            Self::AlreadyResolved => write!(f, "approval already resolved"),
            Self::Expired => write!(f, "approval expired"),
            Self::LimitReached => write!(f, "pending approval limit reached"),
        }
    }
}

impl std::error::Error for ApprovalError {}

/// Bounded, expiry-aware approval store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalStore {
    pending: HashMap<String, ApprovalRequest>,
    max_pending: usize,
}

impl ApprovalStore {
    /// Creates an approval store with a bounded pending count.
    #[must_use]
    pub fn new(max_pending: usize) -> Self {
        Self {
            pending: HashMap::new(),
            max_pending: max_pending.max(1),
        }
    }

    /// Creates one bounded approval request.
    pub fn request(
        &mut self,
        id: &str,
        task_id: &str,
        tool: &str,
        reason: &str,
        requested_at_ms: u64,
        expires_at_ms: u64,
    ) -> Result<(), ApprovalError> {
        if self.pending.contains_key(id) {
            return Err(ApprovalError::AlreadyResolved);
        }
        if self.pending.len() >= self.max_pending {
            return Err(ApprovalError::LimitReached);
        }
        self.pending.insert(
            id.to_owned(),
            ApprovalRequest {
                id: id.to_owned(),
                task_id: task_id.to_owned(),
                tool: tool.to_owned(),
                reason: bounded_reason(reason),
                status: ApprovalStatus::Pending,
                requested_at_ms,
                expires_at_ms,
            },
        );
        Ok(())
    }

    /// Resolves one approval request.
    pub fn decide(
        &mut self,
        id: &str,
        approved: bool,
        now_ms: u64,
    ) -> Result<ApprovalStatus, ApprovalError> {
        let request = self.pending.get_mut(id).ok_or(ApprovalError::NotFound)?;
        if request.status == ApprovalStatus::Expired || now_ms > request.expires_at_ms {
            request.status = ApprovalStatus::Expired;
            return Err(ApprovalError::Expired);
        }
        if request.status != ApprovalStatus::Pending {
            return Err(ApprovalError::AlreadyResolved);
        }
        request.status = if approved {
            ApprovalStatus::Approved
        } else {
            ApprovalStatus::Rejected
        };
        Ok(request.status)
    }

    /// Returns the current status, applying expiry lazily.
    pub fn status(&mut self, id: &str, now_ms: u64) -> Result<ApprovalStatus, ApprovalError> {
        let request = self.pending.get_mut(id).ok_or(ApprovalError::NotFound)?;
        if request.status == ApprovalStatus::Pending && now_ms > request.expires_at_ms {
            request.status = ApprovalStatus::Expired;
        }
        Ok(request.status)
    }
}

/// Bounded append-only event log.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EventLog {
    events: Vec<Event>,
    capacity: usize,
    next_sequence: u64,
    last_timestamp_ms: u64,
}

impl EventLog {
    /// Creates an event log with a hard capacity.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            events: Vec::new(),
            capacity: capacity.max(1),
            next_sequence: 1,
            last_timestamp_ms: 0,
        }
    }

    /// Syncs the durable sequence and timestamp watermark after restoration.
    ///
    /// The in-memory log is a bounded recent-event cache, so its watermark can
    /// advance beyond the retained oldest event.
    pub fn set_watermark(&mut self, last_sequence: u64, last_timestamp_ms: u64) {
        self.next_sequence = self.next_sequence.max(last_sequence.saturating_add(1));
        self.last_timestamp_ms = self.last_timestamp_ms.max(last_timestamp_ms);
    }

    /// Prepares a checksummed event without committing it.
    pub fn prepare_event(
        &self,
        task_id: &str,
        event_type: EventType,
        payload: &str,
        timestamp_ms: u64,
    ) -> Result<Event, EventLogError> {
        if timestamp_ms < self.last_timestamp_ms {
            return Err(EventLogError::NonMonotonicTimestamp);
        }
        let checksum = Event::compute_checksum(
            self.next_sequence,
            task_id,
            event_type,
            payload,
            timestamp_ms,
        );
        Ok(Event {
            sequence: self.next_sequence,
            task_id: task_id.to_owned(),
            event_type,
            payload: payload.to_owned(),
            timestamp_ms,
            checksum,
        })
    }

    /// Commits an already prepared and externally durable event.
    pub fn commit_prepared(&mut self, event: Event) -> Result<(), EventLogError> {
        if event.sequence != self.next_sequence {
            return Err(EventLogError::NonMonotonicSequence);
        }
        if event.timestamp_ms < self.last_timestamp_ms {
            return Err(EventLogError::NonMonotonicTimestamp);
        }
        if event.checksum
            != Event::compute_checksum(
                event.sequence,
                &event.task_id,
                event.event_type,
                &event.payload,
                event.timestamp_ms,
            )
        {
            return Err(EventLogError::ChecksumMismatch);
        }
        if self.events.len() >= self.capacity {
            self.events.remove(0);
        }
        self.events.push(event);
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.last_timestamp_ms = self.events.last().map_or(0, |event| event.timestamp_ms);
        Ok(())
    }

    /// Appends a redacted event and returns its sequence.
    pub fn append(
        &mut self,
        task_id: &str,
        event_type: EventType,
        payload: &str,
        timestamp_ms: u64,
    ) -> Result<u64, EventLogError> {
        let event = self.prepare_event(task_id, event_type, payload, timestamp_ms)?;
        let sequence = event.sequence;
        self.commit_prepared(event)?;
        Ok(sequence)
    }

    /// Verifies monotonicity and checksums.
    pub fn verify(&self) -> Result<(), EventLogError> {
        let mut expected_sequence = self.events.first().map_or(1, |event| event.sequence);
        for event in &self.events {
            if event.sequence != expected_sequence {
                return Err(EventLogError::NonMonotonicSequence);
            }
            let expected = Event::compute_checksum(
                event.sequence,
                &event.task_id,
                event.event_type,
                &event.payload,
                event.timestamp_ms,
            );
            if event.checksum != expected {
                return Err(EventLogError::ChecksumMismatch);
            }
            expected_sequence += 1;
        }
        Ok(())
    }

    /// Returns the stored events.
    #[must_use]
    pub fn events(&self) -> &[Event] {
        &self.events
    }
}

/// Task supervisor and in-memory runtime state.
#[derive(Debug)]
pub struct TaskSupervisor {
    admission: AdmissionController,
    tasks: HashMap<String, Task>,
    event_log: EventLog,
    durable_event_sink: Option<Box<dyn EventSink>>,
}

impl TaskSupervisor {
    /// Creates a supervisor with the supplied controller and event capacity.
    #[must_use]
    pub fn new(admission: AdmissionController, event_capacity: usize) -> Self {
        Self {
            admission,
            tasks: HashMap::new(),
            event_log: EventLog::new(event_capacity),
            durable_event_sink: None,
        }
    }

    /// Creates a supervisor that commits events to a durable write-ahead sink
    /// before applying in-memory task mutations.
    pub fn with_event_sink(
        admission: AdmissionController,
        event_capacity: usize,
        sink: Box<dyn EventSink>,
    ) -> Self {
        Self {
            admission,
            tasks: HashMap::new(),
            event_log: EventLog::new(event_capacity),
            durable_event_sink: Some(sink),
        }
    }

    /// Restores task state after a durable event log has been replayed.
    pub fn restore_tasks(&mut self, tasks: Vec<Task>) -> Result<(), CoreError> {
        for task in tasks {
            task.budget.validate()?;
            if task.usage.elapsed_ms > task.budget.max_duration_ms {
                return Err(CoreError::BudgetExceeded("duration"));
            }
            if self.tasks.contains_key(&task.id) {
                return Err(CoreError::DuplicateTask(task.id));
            }
            self.tasks.insert(task.id.clone(), task);
        }
        Ok(())
    }

    /// Syncs the supervisor's bounded event cache to a durable event watermark.
    pub fn sync_event_watermark(&mut self, last_sequence: u64, last_timestamp_ms: u64) {
        self.event_log
            .set_watermark(last_sequence, last_timestamp_ms);
    }

    /// Returns the number of tasks currently counted as active.
    #[must_use]
    fn active_task_count(&self) -> u32 {
        self.tasks
            .values()
            .filter(|task| {
                matches!(
                    task.status,
                    TaskStatus::Pending
                        | TaskStatus::Planning
                        | TaskStatus::Running
                        | TaskStatus::WaitingModel
                        | TaskStatus::WaitingTool
                        | TaskStatus::WaitingApproval
                        | TaskStatus::Degraded
                )
            })
            .count()
            .try_into()
            .unwrap_or(u32::MAX)
    }

    /// Commits one durable terminal event and then mutates the in-memory task.
    fn commit_terminal_transition(
        &mut self,
        task_id: &str,
        next: TaskStatus,
        event_type: EventType,
        reason: &str,
        now_ms: u64,
    ) -> Result<&Task, CoreError> {
        let task = self
            .tasks
            .get(task_id)
            .ok_or_else(|| CoreError::UnknownTask(task_id.to_owned()))?;
        let mut validated = task.clone();
        validated.transition(next, now_ms)?;
        let reason = bounded_reason(reason);
        self.append_event(task_id, event_type, &reason, now_ms)?;

        let task = self
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| CoreError::UnknownTask(task_id.to_owned()))?;
        task.transition(next, now_ms)?;
        self.tasks
            .get(task_id)
            .ok_or_else(|| CoreError::UnknownTask(task_id.to_owned()))
    }

    /// Durably cancels a task with a bounded, already-redacted reason.
    pub fn cancel_task(
        &mut self,
        task_id: &str,
        reason: &str,
        now_ms: u64,
    ) -> Result<&Task, CoreError> {
        self.commit_terminal_transition(
            task_id,
            TaskStatus::Cancelled,
            EventType::TaskCancelled,
            reason,
            now_ms,
        )
    }

    /// Durably fails a task with a bounded, already-redacted reason.
    pub fn fail_task(
        &mut self,
        task_id: &str,
        reason: &str,
        now_ms: u64,
    ) -> Result<&Task, CoreError> {
        self.commit_terminal_transition(
            task_id,
            TaskStatus::Failed,
            EventType::TaskFailed,
            reason,
            now_ms,
        )
    }

    /// Runs one bounded synchronous model adapter step.
    ///
    /// The adapter receives the shared cancellation signal and must poll it
    /// during long-running generation. A result returned after cancellation is
    /// discarded rather than charged to the model budget.
    pub fn execute_model_step<F>(
        &mut self,
        task_id: &str,
        signal: &ExecutionSignal,
        started_at_ms: u64,
        operation: F,
    ) -> Result<StepOutcome<u64>, CoreError>
    where
        F: FnOnce(&ExecutionSignal) -> Result<ModelStepCompletion, CoreError>,
    {
        let task = self
            .tasks
            .get(task_id)
            .ok_or_else(|| CoreError::UnknownTask(task_id.to_owned()))?;
        if !task.can_execute_model_step() {
            return Err(CoreError::InvalidStatus {
                current: task.status,
                operation: "model step",
            });
        }
        if signal.is_cancelled() {
            self.cancel_task(task_id, "user cancellation", started_at_ms)?;
            return Ok(StepOutcome::Cancelled);
        }
        if task.deadline_exceeded(started_at_ms)? {
            self.fail_task(task_id, "model timeout", started_at_ms)?;
            return Ok(StepOutcome::Failed);
        }

        let completion = match operation(signal) {
            Ok(completion) => completion,
            Err(_) => {
                if signal.is_cancelled() {
                    self.cancel_task(task_id, "user cancellation", started_at_ms)?;
                    return Ok(StepOutcome::Cancelled);
                }
                self.fail_task(task_id, "model step failed", started_at_ms)?;
                return Ok(StepOutcome::Failed);
            }
        };

        if signal.is_cancelled() {
            self.cancel_task(task_id, "user cancellation", completion.completed_at_ms)?;
            return Ok(StepOutcome::Cancelled);
        }
        let task = self
            .tasks
            .get(task_id)
            .ok_or_else(|| CoreError::UnknownTask(task_id.to_owned()))?;
        if task.deadline_exceeded(completion.completed_at_ms)? {
            self.fail_task(task_id, "model timeout", completion.completed_at_ms)?;
            return Ok(StepOutcome::Failed);
        }

        let ModelStepCompletion {
            tokens,
            completed_at_ms,
        } = completion;
        self.consume_model_step(task_id, tokens, completed_at_ms)?;
        Ok(StepOutcome::Completed(tokens))
    }

    /// Runs one bounded synchronous tool adapter step.
    ///
    /// Native tool adapters must poll the supplied signal during I/O or other
    /// blocking work. A late tool result is discarded and not charged to the
    /// task's tool-call budget.
    pub fn execute_tool_step<F>(
        &mut self,
        task_id: &str,
        signal: &ExecutionSignal,
        started_at_ms: u64,
        operation: F,
    ) -> Result<StepOutcome<()>, CoreError>
    where
        F: FnOnce(&ExecutionSignal) -> Result<StepCompletion, CoreError>,
    {
        let task = self
            .tasks
            .get(task_id)
            .ok_or_else(|| CoreError::UnknownTask(task_id.to_owned()))?;
        if !task.can_execute_tool_step() {
            return Err(CoreError::InvalidStatus {
                current: task.status,
                operation: "tool step",
            });
        }
        if signal.is_cancelled() {
            self.cancel_task(task_id, "user cancellation", started_at_ms)?;
            return Ok(StepOutcome::Cancelled);
        }
        if task.deadline_exceeded(started_at_ms)? {
            self.fail_task(task_id, "tool timeout", started_at_ms)?;
            return Ok(StepOutcome::Failed);
        }

        let completion = match operation(signal) {
            Ok(completion) => completion,
            Err(_) => {
                if signal.is_cancelled() {
                    self.cancel_task(task_id, "user cancellation", started_at_ms)?;
                    return Ok(StepOutcome::Cancelled);
                }
                self.fail_task(task_id, "tool step failed", started_at_ms)?;
                return Ok(StepOutcome::Failed);
            }
        };

        if signal.is_cancelled() {
            self.cancel_task(task_id, "user cancellation", completion.completed_at_ms)?;
            return Ok(StepOutcome::Cancelled);
        }
        let task = self
            .tasks
            .get(task_id)
            .ok_or_else(|| CoreError::UnknownTask(task_id.to_owned()))?;
        if task.deadline_exceeded(completion.completed_at_ms)? {
            self.fail_task(task_id, "tool timeout", completion.completed_at_ms)?;
            return Ok(StepOutcome::Failed);
        }

        self.consume_tool_call(task_id, completion.completed_at_ms)?;
        Ok(StepOutcome::Completed(()))
    }

    /// Commits an event to the durable sink before the bounded in-memory cache.
    fn append_event(
        &mut self,
        task_id: &str,
        event_type: EventType,
        payload: &str,
        timestamp_ms: u64,
    ) -> Result<Event, CoreError> {
        let event = self
            .event_log
            .prepare_event(task_id, event_type, payload, timestamp_ms)
            .map_err(CoreError::EventLog)?;
        if let Some(sink) = self.durable_event_sink.as_mut() {
            sink.append_event(&event).map_err(CoreError::EventSink)?;
        }
        self.event_log
            .commit_prepared(event.clone())
            .map_err(CoreError::EventLog)?;
        Ok(event)
    }

    /// Creates a task after admission control.
    pub fn create_task(&mut self, request: CreateTaskRequest<'_>) -> Result<&Task, CoreError> {
        let CreateTaskRequest {
            id,
            session_id,
            goal,
            task_class,
            budget: requested,
            needs_model,
            snapshot,
            now_ms,
        } = request;

        if self.tasks.contains_key(id) {
            return Err(CoreError::DuplicateTask(id.to_owned()));
        }
        let mut snapshot = *snapshot;
        snapshot.active_tasks = snapshot.active_tasks.max(self.active_task_count());
        if let AdmissionDecision::Rejected(reason) =
            self.admission
                .evaluate(task_class, requested, needs_model, &snapshot)
        {
            return Err(CoreError::BudgetExceeded(reason));
        }
        requested.validate()?;
        let task = Task {
            id: id.to_owned(),
            session_id: session_id.to_owned(),
            goal: goal.to_owned(),
            task_class,
            status: TaskStatus::Pending,
            budget: *requested,
            usage: TaskUsage::default(),
            created_at: now_ms,
            updated_at: now_ms,
        };
        let task_payload = serde_json::to_string(&task)
            .map_err(|error| CoreError::Serialization(error.to_string()))?;
        self.append_event(id, EventType::TaskCreated, &task_payload, now_ms)?;
        self.tasks.insert(id.to_owned(), task);
        self.tasks
            .get(id)
            .ok_or_else(|| CoreError::UnknownTask(id.to_owned()))
    }

    /// Transitions a task and records the state event.
    pub fn transition(
        &mut self,
        task_id: &str,
        next: TaskStatus,
        now_ms: u64,
    ) -> Result<&Task, CoreError> {
        let task = self
            .tasks
            .get(task_id)
            .ok_or_else(|| CoreError::UnknownTask(task_id.to_owned()))?;
        let mut validated = task.clone();
        validated.transition(next, now_ms)?;
        self.append_event(task_id, EventType::StateChanged, next.wire_name(), now_ms)?;
        let task = self
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| CoreError::UnknownTask(task_id.to_owned()))?;
        task.transition(next, now_ms)?;
        self.tasks
            .get(task_id)
            .ok_or_else(|| CoreError::UnknownTask(task_id.to_owned()))
    }

    /// Records model token usage against the task budget.
    pub fn consume_model_step(
        &mut self,
        task_id: &str,
        tokens: u64,
        now_ms: u64,
    ) -> Result<&Task, CoreError> {
        let task = self
            .tasks
            .get(task_id)
            .ok_or_else(|| CoreError::UnknownTask(task_id.to_owned()))?;
        let mut validated = task.clone();
        validated.consume_model_step(tokens, now_ms)?;
        self.append_event(
            task_id,
            EventType::ModelStepConsumed,
            &format!("{tokens}"),
            now_ms,
        )?;
        let task = self
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| CoreError::UnknownTask(task_id.to_owned()))?;
        task.consume_model_step(tokens, now_ms)?;
        self.tasks
            .get(task_id)
            .ok_or_else(|| CoreError::UnknownTask(task_id.to_owned()))
    }

    /// Records a tool call against the task budget.
    pub fn consume_tool_call(&mut self, task_id: &str, now_ms: u64) -> Result<&Task, CoreError> {
        let task = self
            .tasks
            .get(task_id)
            .ok_or_else(|| CoreError::UnknownTask(task_id.to_owned()))?;
        let mut validated = task.clone();
        validated.consume_tool_call(now_ms)?;
        self.append_event(task_id, EventType::ToolCallConsumed, "tool-call", now_ms)?;
        let task = self
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| CoreError::UnknownTask(task_id.to_owned()))?;
        task.consume_tool_call(now_ms)?;
        self.tasks
            .get(task_id)
            .ok_or_else(|| CoreError::UnknownTask(task_id.to_owned()))
    }

    /// Returns a task by identifier.
    #[must_use]
    pub fn task(&self, task_id: &str) -> Option<&Task> {
        self.tasks.get(task_id)
    }

    /// Returns all tasks owned by this supervisor.
    #[must_use]
    pub fn tasks(&self) -> Vec<Task> {
        self.tasks.values().cloned().collect()
    }

    /// Returns the append-only event log.
    #[must_use]
    pub fn events(&self) -> &[Event] {
        self.event_log.events()
    }

    /// Verifies event log integrity.
    pub fn verify_events(&self) -> Result<(), CoreError> {
        self.event_log.verify().map_err(CoreError::EventLog)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct FailingSink;

    impl EventSink for FailingSink {
        fn append_event(&mut self, _event: &Event) -> Result<(), EventSinkError> {
            Err(EventSinkError::External("test failure".to_owned()))
        }
    }

    fn snapshot() -> ResourceSnapshot {
        ResourceSnapshot {
            mode: ExecutionMode::Park,
            available_memory_bytes: 256 * 1024 * 1024,
            available_storage_bytes: 64 * 1024 * 1024,
            queue_depth: 0,
            active_tasks: 0,
            thermal_state: ThermalState::Nominal,
            battery_percent: 80,
            charging: false,
            model_available: true,
        }
    }

    #[test]
    fn rejects_invalid_transition() {
        let mut task = Task {
            id: "task-1".into(),
            session_id: "session-1".into(),
            goal: "test".into(),
            task_class: TaskClass::FastQa,
            status: TaskStatus::Pending,
            budget: TaskBudget::fast_qa(),
            usage: TaskUsage::default(),
            created_at: 1,
            updated_at: 1,
        };

        assert_eq!(
            task.transition(TaskStatus::Completed, 2),
            Err(CoreError::InvalidTransition {
                from: TaskStatus::Pending,
                to: TaskStatus::Completed
            })
        );
    }

    #[test]
    fn admission_rejects_long_task_while_driving() {
        let controller = AdmissionController::conservative();
        let mut state = snapshot();
        state.mode = ExecutionMode::DriveHighSpeed;
        let decision =
            controller.evaluate(TaskClass::LongTask, &TaskBudget::long_task(), true, &state);

        assert_eq!(
            decision,
            AdmissionDecision::Rejected("execution mode does not allow this task")
        );
    }

    #[test]
    fn admission_rejects_model_task_without_runtime() {
        let controller = AdmissionController::conservative();
        let mut state = snapshot();
        state.model_available = false;
        let decision = controller.evaluate(TaskClass::FastQa, &TaskBudget::fast_qa(), true, &state);

        assert_eq!(
            decision,
            AdmissionDecision::Rejected("model runtime unavailable")
        );
    }

    #[test]
    fn admission_rejects_when_thermal_is_hot() {
        let controller = AdmissionController::conservative();
        let mut state = snapshot();
        state.thermal_state = ThermalState::Hot;

        assert_eq!(
            controller.evaluate(
                TaskClass::UltraFast,
                &TaskBudget::ultra_fast(),
                false,
                &state,
            ),
            AdmissionDecision::Rejected("thermal state is too high")
        );
    }

    #[test]
    fn admission_rejects_when_battery_is_low() {
        let controller = AdmissionController::conservative();
        let mut state = snapshot();
        state.battery_percent = 10;
        state.charging = false;

        assert_eq!(
            controller.evaluate(
                TaskClass::UltraFast,
                &TaskBudget::ultra_fast(),
                false,
                &state,
            ),
            AdmissionDecision::Rejected("battery is too low")
        );
    }

    #[test]
    fn supervisor_enforces_tool_budget_and_records_events() {
        let mut supervisor = TaskSupervisor::new(AdmissionController::conservative(), 16);
        let budget = TaskBudget::ultra_fast();
        let snapshot = snapshot();
        let request = CreateTaskRequest {
            id: "task-1",
            session_id: "session-1",
            goal: "Read status",
            task_class: TaskClass::UltraFast,
            budget: &budget,
            needs_model: false,
            snapshot: &snapshot,
            now_ms: 100,
        };
        supervisor
            .create_task(request)
            .expect("task should be admitted");
        supervisor
            .transition("task-1", TaskStatus::Planning, 101)
            .expect("planning transition");
        supervisor
            .transition("task-1", TaskStatus::Running, 102)
            .expect("running transition");
        supervisor
            .consume_tool_call("task-1", 103)
            .expect("first tool call");

        assert_eq!(
            supervisor.consume_tool_call("task-1", 104),
            Err(CoreError::BudgetExceeded("tool calls"))
        );
        assert_eq!(supervisor.events().len(), 4);
        supervisor.verify_events().expect("event log should verify");
    }

    #[test]
    fn duration_budget_fails_before_accounting_usage() {
        let mut supervisor = TaskSupervisor::new(AdmissionController::conservative(), 16);
        let budget = TaskBudget::ultra_fast();
        let snapshot = snapshot();
        let request = CreateTaskRequest {
            id: "task-duration",
            session_id: "session-1",
            goal: "Fast command",
            task_class: TaskClass::UltraFast,
            budget: &budget,
            needs_model: false,
            snapshot: &snapshot,
            now_ms: 1_000,
        };
        supervisor.create_task(request).expect("task admitted");
        supervisor
            .transition("task-duration", TaskStatus::Planning, 1_001)
            .expect("planning transition");
        supervisor
            .transition("task-duration", TaskStatus::Running, 1_002)
            .expect("running transition");

        let before = supervisor.task("task-duration").expect("task exists").usage;
        let result = supervisor
            .consume_model_step("task-duration", 32, 2_501)
            .map(|_| ());
        let after = supervisor.task("task-duration").expect("task exists").usage;

        assert_eq!(result, Err(CoreError::BudgetExceeded("duration")));
        assert_eq!(before, after);
    }

    #[test]
    fn tool_calls_are_rejected_after_completion() {
        let mut supervisor = TaskSupervisor::new(AdmissionController::conservative(), 16);
        let budget = TaskBudget::ultra_fast();
        let snapshot = snapshot();
        let request = CreateTaskRequest {
            id: "task-finished",
            session_id: "session-1",
            goal: "Fast command",
            task_class: TaskClass::UltraFast,
            budget: &budget,
            needs_model: false,
            snapshot: &snapshot,
            now_ms: 1_000,
        };
        supervisor.create_task(request).expect("task admitted");
        supervisor
            .transition("task-finished", TaskStatus::Planning, 1_001)
            .expect("planning transition");
        supervisor
            .transition("task-finished", TaskStatus::Running, 1_002)
            .expect("running transition");
        supervisor
            .transition("task-finished", TaskStatus::Completed, 1_003)
            .expect("completion transition");

        assert_eq!(
            supervisor.consume_tool_call("task-finished", 1_004),
            Err(CoreError::InvalidStatus {
                current: TaskStatus::Completed,
                operation: "tool call"
            })
        );
    }

    #[test]
    fn supervisor_derives_active_task_count_for_admission() {
        let mut supervisor = TaskSupervisor::new(AdmissionController::conservative(), 16);
        let budget = TaskBudget::ultra_fast();
        let snapshot = snapshot();
        let first = CreateTaskRequest {
            id: "task-1",
            session_id: "session-1",
            goal: "First",
            task_class: TaskClass::UltraFast,
            budget: &budget,
            needs_model: false,
            snapshot: &snapshot,
            now_ms: 1_000,
        };
        supervisor.create_task(first).expect("first task admitted");
        supervisor
            .transition("task-1", TaskStatus::Planning, 1_001)
            .expect("planning transition");

        let second = CreateTaskRequest {
            id: "task-2",
            session_id: "session-1",
            goal: "Second",
            task_class: TaskClass::UltraFast,
            budget: &budget,
            needs_model: false,
            snapshot: &snapshot,
            now_ms: 1_002,
        };

        assert_eq!(
            supervisor.create_task(second).map(|_| ()),
            Err(CoreError::BudgetExceeded("active task limit reached"))
        );
        assert!(supervisor.task("task-2").is_none());
    }

    #[test]
    fn durable_event_failure_leaves_task_unchanged() {
        let mut supervisor = TaskSupervisor::with_event_sink(
            AdmissionController::conservative(),
            16,
            Box::new(FailingSink),
        );
        let budget = TaskBudget::ultra_fast();
        let snapshot = snapshot();
        let request = CreateTaskRequest {
            id: "task-durable",
            session_id: "session-1",
            goal: "Should not mutate",
            task_class: TaskClass::UltraFast,
            budget: &budget,
            needs_model: false,
            snapshot: &snapshot,
            now_ms: 1_000,
        };

        let result = supervisor.create_task(request).map(|_| ());

        assert!(matches!(
            result,
            Err(CoreError::EventSink(EventSinkError::External(_)))
        ));
        assert!(supervisor.task("task-durable").is_none());
        assert!(supervisor.events().is_empty());
    }

    #[test]
    fn bounded_model_step_accounts_tokens_when_not_canceled() {
        let mut supervisor = TaskSupervisor::new(AdmissionController::conservative(), 16);
        let budget = TaskBudget::ultra_fast();
        let snapshot = snapshot();
        let request = CreateTaskRequest {
            id: "task-model-step",
            session_id: "session-1",
            goal: "Run bounded model",
            task_class: TaskClass::UltraFast,
            budget: &budget,
            needs_model: true,
            snapshot: &snapshot,
            now_ms: 1_000,
        };
        supervisor.create_task(request).expect("task admitted");
        supervisor
            .transition("task-model-step", TaskStatus::Planning, 1_001)
            .expect("planning transition");
        supervisor
            .transition("task-model-step", TaskStatus::Running, 1_002)
            .expect("running transition");

        let signal = ExecutionSignal::new();
        let outcome = supervisor
            .execute_model_step("task-model-step", &signal, 1_003, |_| {
                Ok(ModelStepCompletion {
                    tokens: 32,
                    completed_at_ms: 1_004,
                })
            })
            .expect("model step should settle");

        assert_eq!(outcome, StepOutcome::Completed(32));
        let task = supervisor.task("task-model-step").expect("task exists");
        assert_eq!(task.status, TaskStatus::Running);
        assert_eq!(task.usage.model_tokens, 32);
    }

    #[test]
    fn bounded_model_step_discards_late_result_after_cancellation() {
        let mut supervisor = TaskSupervisor::new(AdmissionController::conservative(), 16);
        let budget = TaskBudget::ultra_fast();
        let snapshot = snapshot();
        let request = CreateTaskRequest {
            id: "task-late-cancel",
            session_id: "session-1",
            goal: "Discard late model result",
            task_class: TaskClass::UltraFast,
            budget: &budget,
            needs_model: true,
            snapshot: &snapshot,
            now_ms: 1_000,
        };
        supervisor.create_task(request).expect("task admitted");
        supervisor
            .transition("task-late-cancel", TaskStatus::Planning, 1_001)
            .expect("planning transition");
        supervisor
            .transition("task-late-cancel", TaskStatus::Running, 1_002)
            .expect("running transition");

        let signal = ExecutionSignal::new();
        let outcome = supervisor
            .execute_model_step("task-late-cancel", &signal, 1_003, |signal| {
                signal.cancel();
                Ok(ModelStepCompletion {
                    tokens: 64,
                    completed_at_ms: 1_004,
                })
            })
            .expect("canceled model step should settle");

        assert_eq!(outcome, StepOutcome::Cancelled);
        let task = supervisor.task("task-late-cancel").expect("task exists");
        assert_eq!(task.status, TaskStatus::Cancelled);
        assert_eq!(task.usage.model_tokens, 0);
        assert_eq!(
            supervisor.events().last().map(|event| event.event_type),
            Some(EventType::TaskCancelled)
        );
    }

    #[test]
    fn bounded_model_step_times_out_before_accounting() {
        let mut supervisor = TaskSupervisor::new(AdmissionController::conservative(), 16);
        let budget = TaskBudget::ultra_fast();
        let snapshot = snapshot();
        let request = CreateTaskRequest {
            id: "task-timeout",
            session_id: "session-1",
            goal: "Time out model step",
            task_class: TaskClass::UltraFast,
            budget: &budget,
            needs_model: true,
            snapshot: &snapshot,
            now_ms: 1_000,
        };
        supervisor.create_task(request).expect("task admitted");
        supervisor
            .transition("task-timeout", TaskStatus::Planning, 1_001)
            .expect("planning transition");
        supervisor
            .transition("task-timeout", TaskStatus::Running, 1_002)
            .expect("running transition");

        let signal = ExecutionSignal::new();
        let outcome = supervisor
            .execute_model_step("task-timeout", &signal, 1_003, |_| {
                Ok(ModelStepCompletion {
                    tokens: 64,
                    completed_at_ms: 2_501,
                })
            })
            .expect("timeout model step should settle");

        assert_eq!(outcome, StepOutcome::Failed);
        let task = supervisor.task("task-timeout").expect("task exists");
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(task.usage.model_tokens, 0);
        assert_eq!(
            supervisor.events().last().map(|event| event.event_type),
            Some(EventType::TaskFailed)
        );
    }

    #[test]
    fn bounded_tool_step_discards_late_result_after_cancellation() {
        let mut supervisor = TaskSupervisor::new(AdmissionController::conservative(), 16);
        let budget = TaskBudget::ultra_fast();
        let snapshot = snapshot();
        let request = CreateTaskRequest {
            id: "task-tool-cancel",
            session_id: "session-1",
            goal: "Discard late tool result",
            task_class: TaskClass::UltraFast,
            budget: &budget,
            needs_model: false,
            snapshot: &snapshot,
            now_ms: 1_000,
        };
        supervisor.create_task(request).expect("task admitted");
        supervisor
            .transition("task-tool-cancel", TaskStatus::Planning, 1_001)
            .expect("planning transition");
        supervisor
            .transition("task-tool-cancel", TaskStatus::Running, 1_002)
            .expect("running transition");

        let signal = ExecutionSignal::new();
        let outcome = supervisor
            .execute_tool_step("task-tool-cancel", &signal, 1_003, |signal| {
                signal.cancel();
                Ok(StepCompletion {
                    completed_at_ms: 1_004,
                })
            })
            .expect("canceled tool step should settle");

        assert_eq!(outcome, StepOutcome::Cancelled);
        let task = supervisor.task("task-tool-cancel").expect("task exists");
        assert_eq!(task.status, TaskStatus::Cancelled);
        assert_eq!(task.usage.tool_calls, 0);
    }

    #[test]
    fn approval_store_is_bounded_and_expiry_aware() {
        let mut store = ApprovalStore::new(1);
        store
            .request(
                "approval-1",
                "task-1",
                "local.write",
                "reason",
                1_000,
                2_000,
            )
            .expect("approval should be created");

        assert_eq!(
            store.request(
                "approval-2",
                "task-2",
                "local.write",
                "reason",
                1_000,
                2_000
            ),
            Err(ApprovalError::LimitReached)
        );
        assert_eq!(
            store.status("approval-1", 2_001),
            Ok(ApprovalStatus::Expired)
        );
        assert_eq!(
            store.decide("approval-1", true, 2_001),
            Err(ApprovalError::Expired)
        );
    }

    #[test]
    fn approval_store_resolves_once() {
        let mut store = ApprovalStore::new(4);
        store
            .request(
                "approval-1",
                "task-1",
                "local.write",
                "reason",
                1_000,
                2_000,
            )
            .expect("approval should be created");

        assert_eq!(
            store.decide("approval-1", true, 1_500),
            Ok(ApprovalStatus::Approved)
        );
        assert_eq!(
            store.decide("approval-1", false, 1_600),
            Err(ApprovalError::AlreadyResolved)
        );
    }

    #[test]
    fn event_log_is_append_only_and_bounded() {
        let mut log = EventLog::new(2);
        assert_eq!(log.append("task", EventType::TaskCreated, "one", 1), Ok(1));
        assert_eq!(log.append("task", EventType::StateChanged, "two", 2), Ok(2));
        assert_eq!(
            log.append("task", EventType::TaskFinished, "three", 3),
            Ok(3)
        );

        assert_eq!(log.events().len(), 2);
        assert_eq!(log.events()[0].sequence, 2);
        assert_eq!(log.events()[1].sequence, 3);
        assert!(log.verify().is_ok());
    }
}

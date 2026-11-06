//! End-to-end supervisor-to-SQLite recovery scenario.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use harness_core::{
    AdmissionController, CreateTaskRequest, ExecutionSignal, ModelStepCompletion, ResourceSnapshot,
    StepOutcome, TaskBudget, TaskStatus, TaskSupervisor, ThermalState,
};
use harness_policy::TaskClass;
use harness_storage::{PersistentTaskSupervisor, TaskStore};

fn snapshot() -> ResourceSnapshot {
    ResourceSnapshot {
        mode: harness_policy::ExecutionMode::Park,
        available_memory_bytes: TaskBudget::fast_qa().max_memory_bytes,
        available_storage_bytes: TaskBudget::fast_qa().max_storage_bytes,
        queue_depth: 0,
        active_tasks: 0,
        thermal_state: ThermalState::Nominal,
        battery_percent: 80,
        charging: false,
        model_available: true,
    }
}

fn temp_path() -> std::path::PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should advance")
        .as_nanos();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "edge-agent-supervisor-recovery-{unique}-{id}.sqlite"
    ))
}

#[test]
fn supervisor_events_replay_across_process_boundary() {
    let path = temp_path();
    let budget = TaskBudget::fast_qa();
    let snapshot = snapshot();
    let request = CreateTaskRequest {
        id: "task-recovery",
        session_id: "session-recovery",
        goal: "Recover a bounded task",
        task_class: TaskClass::FastQa,
        budget: &budget,
        needs_model: true,
        snapshot: &snapshot,
        now_ms: 1_000,
    };

    {
        let mut supervisor = TaskSupervisor::new(AdmissionController::conservative(), 32);
        let mut store = TaskStore::open(&path).expect("store should open");

        let task = supervisor
            .create_task(request)
            .expect("task should be admitted")
            .clone();
        store
            .append_event(supervisor.events().last().expect("creation event"))
            .expect("creation event should persist");
        store
            .save_checkpoint(&task, 1_000)
            .expect("checkpoint should persist");

        for (next, now_ms) in [(TaskStatus::Planning, 1_001), (TaskStatus::Running, 1_002)] {
            supervisor
                .transition("task-recovery", next, now_ms)
                .expect("state transition");
            store
                .append_event(supervisor.events().last().expect("state event"))
                .expect("state event should persist");
        }

        supervisor
            .consume_model_step("task-recovery", 64, 1_003)
            .expect("model step");
        store
            .append_event(supervisor.events().last().expect("model event"))
            .expect("model event should persist");

        supervisor
            .consume_tool_call("task-recovery", 1_004)
            .expect("tool call");
        store
            .append_event(supervisor.events().last().expect("tool event"))
            .expect("tool event should persist");

        supervisor
            .transition("task-recovery", TaskStatus::Completed, 1_005)
            .expect("completion transition");
        store
            .append_event(supervisor.events().last().expect("completion event"))
            .expect("completion event should persist");
    }

    let reopened = TaskStore::open(&path).expect("store should reopen");
    let restored = reopened
        .restore_task("task-recovery")
        .expect("task should restore")
        .expect("checkpoint should exist");

    assert_eq!(restored.status, TaskStatus::Completed);
    assert_eq!(restored.usage.steps, 1);
    assert_eq!(restored.usage.model_tokens, 64);
    assert_eq!(restored.usage.tool_calls, 1);
    assert_eq!(restored.usage.elapsed_ms, 5);
    assert_eq!(restored.updated_at, 1_005);

    std::fs::remove_file(&path).expect("test database should be removed");
}

#[test]
fn persistent_supervisor_cancellation_survives_reopen() {
    let path = temp_path();
    let budget = TaskBudget::fast_qa();
    let snapshot = snapshot();

    {
        let mut supervisor =
            PersistentTaskSupervisor::open(&path, AdmissionController::conservative(), 32)
                .expect("persistent supervisor should open");
        let request = CreateTaskRequest {
            id: "task-cancel",
            session_id: "session-cancel",
            goal: "Cancel durable model step",
            task_class: TaskClass::FastQa,
            budget: &budget,
            needs_model: true,
            snapshot: &snapshot,
            now_ms: 1_000,
        };
        supervisor
            .create_task(request)
            .expect("task should be durable");
        supervisor
            .transition("task-cancel", TaskStatus::Planning, 1_001)
            .expect("planning should be durable");
        supervisor
            .transition("task-cancel", TaskStatus::Running, 1_002)
            .expect("running should be durable");

        let signal = ExecutionSignal::new();
        let outcome = supervisor
            .execute_model_step("task-cancel", &signal, 1_003, |signal| {
                signal.cancel();
                Ok(ModelStepCompletion {
                    tokens: 128,
                    completed_at_ms: 1_004,
                })
            })
            .expect("canceled durable step should settle");

        assert_eq!(outcome, StepOutcome::Cancelled);
    }

    let supervisor = PersistentTaskSupervisor::open(&path, AdmissionController::conservative(), 32)
        .expect("canceled supervisor should reopen");
    let restored = supervisor
        .task("task-cancel")
        .expect("canceled task should restore");
    assert_eq!(restored.status, TaskStatus::Cancelled);
    assert_eq!(restored.usage.model_tokens, 0);

    std::fs::remove_file(&path).expect("test database should be removed");
}

#[test]
fn persistent_supervisor_timeout_survives_reopen() {
    let path = temp_path();
    let budget = TaskBudget::fast_qa();
    let snapshot = snapshot();

    {
        let mut supervisor =
            PersistentTaskSupervisor::open(&path, AdmissionController::conservative(), 32)
                .expect("persistent supervisor should open");
        let request = CreateTaskRequest {
            id: "task-timeout",
            session_id: "session-timeout",
            goal: "Time out durable model step",
            task_class: TaskClass::FastQa,
            budget: &budget,
            needs_model: true,
            snapshot: &snapshot,
            now_ms: 1_000,
        };
        supervisor
            .create_task(request)
            .expect("task should be durable");
        supervisor
            .transition("task-timeout", TaskStatus::Planning, 1_001)
            .expect("planning should be durable");
        supervisor
            .transition("task-timeout", TaskStatus::Running, 1_002)
            .expect("running should be durable");

        let signal = ExecutionSignal::new();
        let outcome = supervisor
            .execute_model_step("task-timeout", &signal, 1_003, |_| {
                Ok(ModelStepCompletion {
                    tokens: 128,
                    completed_at_ms: 6_001,
                })
            })
            .expect("timeout durable step should settle");

        assert_eq!(outcome, StepOutcome::Failed);
    }

    let supervisor = PersistentTaskSupervisor::open(&path, AdmissionController::conservative(), 32)
        .expect("failed supervisor should reopen");
    let restored = supervisor
        .task("task-timeout")
        .expect("failed task should restore");
    assert_eq!(restored.status, TaskStatus::Failed);
    assert_eq!(restored.usage.model_tokens, 0);

    std::fs::remove_file(&path).expect("test database should be removed");
}

#[test]
fn persistent_supervisor_reopens_and_continues() {
    let path = temp_path();
    let budget = TaskBudget::fast_qa();
    let snapshot = snapshot();

    {
        let mut supervisor =
            PersistentTaskSupervisor::open(&path, AdmissionController::conservative(), 32)
                .expect("persistent supervisor should open");
        let request = CreateTaskRequest {
            id: "task-automatic",
            session_id: "session-automatic",
            goal: "Recover automatic persistence",
            task_class: TaskClass::FastQa,
            budget: &budget,
            needs_model: true,
            snapshot: &snapshot,
            now_ms: 1_000,
        };
        supervisor
            .create_task(request)
            .expect("task should be durably created");
        supervisor
            .transition("task-automatic", TaskStatus::Planning, 1_001)
            .expect("planning should be durable");
        supervisor
            .transition("task-automatic", TaskStatus::Running, 1_002)
            .expect("running transition should be durable");
        supervisor
            .consume_model_step("task-automatic", 96, 1_003)
            .expect("model step should be durable");
    }

    let mut supervisor =
        PersistentTaskSupervisor::open(&path, AdmissionController::conservative(), 32)
            .expect("persistent supervisor should reopen");
    let restored = supervisor
        .task("task-automatic")
        .expect("task should be restored");
    assert_eq!(restored.status, TaskStatus::Running);
    assert_eq!(restored.usage.model_tokens, 96);
    assert_eq!(restored.usage.elapsed_ms, 3);

    supervisor
        .transition("task-automatic", TaskStatus::Completed, 1_004)
        .expect("continuation should be durable");
    let restored = supervisor
        .task("task-automatic")
        .expect("task should remain available");
    assert_eq!(restored.status, TaskStatus::Completed);

    std::fs::remove_file(&path).expect("test database should be removed");
}

#[test]
fn persistent_supervisor_compacts_after_checkpoint_and_reopens() {
    let path = temp_path();
    let budget = TaskBudget::fast_qa();
    let snapshot = snapshot();

    {
        let mut supervisor =
            PersistentTaskSupervisor::open(&path, AdmissionController::conservative(), 32)
                .expect("persistent supervisor should open");
        let request = CreateTaskRequest {
            id: "task-compact",
            session_id: "session-compact",
            goal: "Compact durable history",
            task_class: TaskClass::FastQa,
            budget: &budget,
            needs_model: true,
            snapshot: &snapshot,
            now_ms: 1_000,
        };
        supervisor
            .create_task(request)
            .expect("task should be durable");
        supervisor
            .transition("task-compact", TaskStatus::Planning, 1_001)
            .expect("planning should be durable");
        supervisor
            .transition("task-compact", TaskStatus::Running, 1_002)
            .expect("running should be durable");
        supervisor
            .consume_model_step("task-compact", 64, 1_003)
            .expect("model usage should be durable");

        let report = supervisor
            .compact_retention(1_004)
            .expect("retention should compact");
        assert_eq!(report.compacted_tasks, 1);
        assert_eq!(report.removed_events, 3);
        assert_eq!(report.retained_events, 1);
    }

    let mut supervisor =
        PersistentTaskSupervisor::open(&path, AdmissionController::conservative(), 32)
            .expect("compacted supervisor should reopen");
    let restored = supervisor
        .task("task-compact")
        .expect("task should restore from checkpoint");
    assert_eq!(restored.status, TaskStatus::Running);
    assert_eq!(restored.usage.model_tokens, 64);

    supervisor
        .transition("task-compact", TaskStatus::Completed, 1_005)
        .expect("new event should append after compaction");
    let restored = supervisor
        .task("task-compact")
        .expect("task should remain available");
    assert_eq!(restored.status, TaskStatus::Completed);

    std::fs::remove_file(&path).expect("test database should be removed");
}

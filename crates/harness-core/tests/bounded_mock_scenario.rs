//! End-to-end Phase 0 scenario using deterministic mock model and tools.

use std::collections::BTreeSet;

use harness_core::{
    AdmissionController, CreateTaskRequest, ResourceSnapshot, TaskBudget, TaskStatus,
    TaskSupervisor, ThermalState,
};
use harness_model::{MockModelRuntime, ModelOutput, ModelPrompt, ModelRuntime};
use harness_policy::{Capability, ExecutionMode, TaskClass};
use harness_tools::{MockToolGateway, ToolAction, ToolObservation, ToolRegistry};

#[test]
fn high_speed_mock_scenario_stays_bounded_and_auditable() {
    let mut supervisor = TaskSupervisor::new(AdmissionController::conservative(), 32);
    let budget = TaskBudget::fast_qa();
    let snapshot = ResourceSnapshot {
        mode: ExecutionMode::DriveHighSpeed,
        available_memory_bytes: budget.max_memory_bytes,
        available_storage_bytes: budget.max_storage_bytes,
        queue_depth: 0,
        active_tasks: 0,
        thermal_state: ThermalState::Nominal,
        battery_percent: 80,
        charging: false,
        model_available: true,
    };
    let request = CreateTaskRequest {
        id: "task-high-speed",
        session_id: "session-vehicle",
        goal: "Read local assistant status",
        task_class: TaskClass::FastQa,
        budget: &budget,
        needs_model: true,
        snapshot: &snapshot,
        now_ms: 1_000,
    };

    supervisor.create_task(request).expect("task admitted");
    supervisor
        .transition("task-high-speed", TaskStatus::Planning, 1_001)
        .expect("planning transition");
    supervisor
        .transition("task-high-speed", TaskStatus::Running, 1_002)
        .expect("running transition");

    let mut model =
        MockModelRuntime::new(ModelOutput::Action(ToolAction::read_local("status")), 32);
    let response = model
        .generate(ModelPrompt {
            goal: "Read local assistant status",
            max_tokens: budget.max_model_tokens,
        })
        .expect("model output");
    supervisor
        .consume_model_step("task-high-speed", response.tokens, 1_003)
        .expect("model budget");

    let ModelOutput::Action(action) = response.output else {
        panic!("scenario expects a structured tool action");
    };
    let mut tools = MockToolGateway::new(ToolRegistry::phase0());
    tools.seed("status", "ready");
    let granted = BTreeSet::from([Capability::ReadLocal]);
    let task = supervisor
        .task("task-high-speed")
        .expect("task should exist");
    let observation = tools
        .invoke(
            action,
            task.task_class,
            &granted,
            snapshot.mode,
            task.usage.tool_calls,
            false,
        )
        .expect("read-only tool allowed while driving");

    assert_eq!(
        observation,
        ToolObservation::LocalValue {
            key: "status".into(),
            value: "ready".into()
        }
    );
    supervisor
        .consume_tool_call("task-high-speed", 1_004)
        .expect("tool budget");
    supervisor
        .transition("task-high-speed", TaskStatus::Completed, 1_005)
        .expect("completion transition");

    let task = supervisor
        .task("task-high-speed")
        .expect("task should exist");
    assert_eq!(task.status, TaskStatus::Completed);
    assert_eq!(task.usage.steps, 1);
    assert_eq!(task.usage.tool_calls, 1);
    assert_eq!(task.usage.model_tokens, 32);
    assert_eq!(supervisor.events().len(), 6);
    supervisor.verify_events().expect("event log intact");
}

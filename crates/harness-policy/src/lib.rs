//! Deterministic policy primitives for Edge Agent Harness.
//!
//! This crate intentionally has no model or platform dependencies. Policy
//! decisions must be reproducible from the supplied facts.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Vehicle and device execution mode used by policy evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    /// Vehicle is parked.
    Park,
    /// Vehicle is moving at a low speed.
    DriveLowSpeed,
    /// Vehicle is moving at a high speed.
    DriveHighSpeed,
    /// Vehicle is reversing; agent work is disabled.
    Reverse,
    /// Vehicle is charging and can run deferred work.
    Charging,
    /// Device is in a low-power state.
    LowPower,
    /// Platform is busy and agent work should yield.
    SystemBusy,
}

impl ExecutionMode {
    /// Returns whether a task class is allowed in this execution mode.
    #[must_use]
    pub fn allows(self, task_class: TaskClass) -> bool {
        match self {
            Self::Park | Self::Charging => true,
            Self::DriveLowSpeed => matches!(
                task_class,
                TaskClass::UltraFast | TaskClass::FastQa | TaskClass::ReadOnly
            ),
            Self::DriveHighSpeed => {
                matches!(task_class, TaskClass::UltraFast | TaskClass::FastQa)
            }
            Self::Reverse | Self::SystemBusy => false,
            Self::LowPower => task_class == TaskClass::UltraFast,
        }
    }
}

/// Classification used to bound task complexity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskClass {
    /// A deterministic or cached command with no model work.
    UltraFast,
    /// A short answer requiring one model call.
    FastQa,
    /// Read-only retrieval or summarization.
    ReadOnly,
    /// A write action or multi-step task.
    Write,
    /// A long-running task that must be deferred when resources are limited.
    LongTask,
    /// A vehicle API action subject to the vehicle safety gateway.
    VehicleAction,
}

/// Risk level assigned by a tool manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RiskLevel {
    /// Low-risk read or query.
    Low,
    /// Medium-risk local mutation.
    Medium,
    /// High-risk mutation or external transfer.
    High,
    /// Never executable through the harness.
    Forbidden,
}

/// Capabilities that can be granted to a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    /// Read task-local or app-local state.
    ReadLocal,
    /// Write task-local or app-local state.
    WriteLocal,
    /// Transfer data off the device.
    SendNetwork,
    /// Read whitelisted vehicle state.
    VehicleRead,
    /// Request a whitelisted vehicle action.
    VehicleWrite,
    /// Modify system or platform configuration.
    SystemControl,
}

/// A deterministic description of an executable tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolManifest {
    /// Stable tool identifier.
    pub name: &'static str,
    /// Maximum risk assigned to this tool.
    pub risk: RiskLevel,
    /// Capabilities required by this tool.
    pub required_capabilities: &'static [Capability],
    /// Whether this tool may run in any driving mode without further review.
    pub driving_allowed: bool,
    /// Whether user approval is always required.
    pub requires_approval: bool,
    /// Maximum calls allowed for one task.
    pub max_calls_per_task: u32,
}

impl ToolManifest {
    /// Creates a manifest for a low-risk local read tool.
    #[must_use]
    pub const fn local_read(name: &'static str, max_calls_per_task: u32) -> Self {
        Self {
            name,
            risk: RiskLevel::Low,
            required_capabilities: &[Capability::ReadLocal],
            driving_allowed: true,
            requires_approval: false,
            max_calls_per_task,
        }
    }

    /// Creates a manifest for a local write tool requiring approval.
    #[must_use]
    pub const fn local_write(name: &'static str, max_calls_per_task: u32) -> Self {
        Self {
            name,
            risk: RiskLevel::Medium,
            required_capabilities: &[Capability::WriteLocal],
            driving_allowed: false,
            requires_approval: true,
            max_calls_per_task,
        }
    }
}

/// A validated model action awaiting policy evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedAction {
    /// Tool selected by the model.
    pub tool: &'static str,
    /// Task class supplied by the supervisor.
    pub task_class: TaskClass,
}

/// Why a policy decision was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyReason {
    /// The action is allowed.
    Allowed,
    /// The selected tool is not registered.
    UnknownTool,
    /// A required capability was not granted.
    MissingCapability(Capability),
    /// The tool is forbidden.
    ForbiddenRisk,
    /// Execution mode does not allow this task class.
    ModeDisallowed,
    /// The tool is not safe while driving.
    DrivingDisallowed,
}

impl fmt::Display for PolicyReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allowed => write!(f, "allowed"),
            Self::UnknownTool => write!(f, "unknown tool"),
            Self::MissingCapability(capability) => write!(f, "missing capability: {capability:?}"),
            Self::ForbiddenRisk => write!(f, "forbidden risk"),
            Self::ModeDisallowed => write!(f, "execution mode disallowed"),
            Self::DrivingDisallowed => write!(f, "tool disallowed while driving"),
        }
    }
}

/// Result of a deterministic policy evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDecision {
    /// Whether the action may continue.
    pub allowed: bool,
    /// Whether user approval is still required before execution.
    pub requires_approval: bool,
    /// Reasons explaining the decision.
    pub reasons: Vec<PolicyReason>,
}

/// A deterministic policy engine.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PolicyEngine;

impl PolicyEngine {
    /// Evaluates a proposed action against the supplied facts.
    ///
    /// The action is not executed here. The result is an auditable decision
    /// consumed by the core supervisor.
    #[must_use]
    pub fn evaluate(
        &self,
        action: &ProposedAction,
        manifest: &ToolManifest,
        granted: &BTreeSet<Capability>,
        mode: ExecutionMode,
    ) -> PolicyDecision {
        let mut reasons = Vec::new();

        if action.tool != manifest.name {
            reasons.push(PolicyReason::UnknownTool);
        }
        if manifest.risk == RiskLevel::Forbidden {
            reasons.push(PolicyReason::ForbiddenRisk);
        }
        if !mode.allows(action.task_class) {
            reasons.push(PolicyReason::ModeDisallowed);
        }
        if matches!(
            mode,
            ExecutionMode::DriveLowSpeed | ExecutionMode::DriveHighSpeed | ExecutionMode::Reverse
        ) && !manifest.driving_allowed
        {
            reasons.push(PolicyReason::DrivingDisallowed);
        }
        for capability in manifest.required_capabilities {
            if !granted.contains(capability) {
                reasons.push(PolicyReason::MissingCapability(*capability));
            }
        }

        let allowed = reasons.is_empty();
        PolicyDecision {
            allowed,
            requires_approval: allowed && manifest.requires_approval,
            reasons,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_action() -> ProposedAction {
        ProposedAction {
            tool: "local.read",
            task_class: TaskClass::ReadOnly,
        }
    }

    #[test]
    fn rejects_missing_capability() {
        let manifest = ToolManifest::local_read("local.read", 3);
        let decision = PolicyEngine.evaluate(
            &read_action(),
            &manifest,
            &BTreeSet::new(),
            ExecutionMode::Park,
        );

        assert!(!decision.allowed);
        assert_eq!(
            decision.reasons,
            vec![PolicyReason::MissingCapability(Capability::ReadLocal)]
        );
    }

    #[test]
    fn high_speed_allows_only_bounded_interactions() {
        assert!(ExecutionMode::DriveHighSpeed.allows(TaskClass::UltraFast));
        assert!(ExecutionMode::DriveHighSpeed.allows(TaskClass::FastQa));
        assert!(!ExecutionMode::DriveHighSpeed.allows(TaskClass::ReadOnly));
        assert!(!ExecutionMode::DriveHighSpeed.allows(TaskClass::Write));
        assert!(!ExecutionMode::Reverse.allows(TaskClass::UltraFast));
    }

    #[test]
    fn local_write_requires_approval() {
        let mut granted = BTreeSet::new();
        granted.insert(Capability::WriteLocal);
        let manifest = ToolManifest::local_write("local.write", 1);
        let decision = PolicyEngine.evaluate(
            &ProposedAction {
                tool: "local.write",
                task_class: TaskClass::Write,
            },
            &manifest,
            &granted,
            ExecutionMode::Park,
        );

        assert!(decision.allowed);
        assert!(decision.requires_approval);
    }
}

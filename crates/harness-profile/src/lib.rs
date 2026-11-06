//! Device tiers and constrained task budget selection.

use harness_core::TaskBudget;
use harness_policy::TaskClass;

/// Device capability tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceTier {
    /// High-memory device with model acceleration.
    Flagship,
    /// Mainstream phone or IVI tier.
    Mainstream,
    /// Entry-level phone or low-memory IVI tier.
    Entry,
}

/// Device capability profile used to constrain task budgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceProfile {
    /// Capability tier.
    pub tier: DeviceTier,
    /// Maximum task memory reserved in bytes.
    pub max_task_memory_bytes: u64,
    /// Maximum task-local storage in bytes.
    pub max_task_storage_bytes: u64,
    /// Whether a local model runtime is expected.
    pub model_available: bool,
    /// Whether thermal sensitivity should downgrade budgets.
    pub thermal_sensitive: bool,
}

impl DeviceProfile {
    /// Creates a flagship profile.
    #[must_use]
    pub const fn flagship() -> Self {
        Self {
            tier: DeviceTier::Flagship,
            max_task_memory_bytes: 128 * 1024 * 1024,
            max_task_storage_bytes: 16 * 1024 * 1024,
            model_available: true,
            thermal_sensitive: false,
        }
    }

    /// Creates a mainstream profile.
    #[must_use]
    pub const fn mainstream() -> Self {
        Self {
            tier: DeviceTier::Mainstream,
            max_task_memory_bytes: 64 * 1024 * 1024,
            max_task_storage_bytes: 8 * 1024 * 1024,
            model_available: true,
            thermal_sensitive: false,
        }
    }

    /// Creates an entry profile.
    #[must_use]
    pub const fn entry() -> Self {
        Self {
            tier: DeviceTier::Entry,
            max_task_memory_bytes: 32 * 1024 * 1024,
            max_task_storage_bytes: 4 * 1024 * 1024,
            model_available: false,
            thermal_sensitive: true,
        }
    }

    /// Returns a device-constrained budget for a task class.
    #[must_use]
    pub fn budget_for(&self, task_class: TaskClass) -> TaskBudget {
        let base = match task_class {
            TaskClass::UltraFast => TaskBudget::ultra_fast(),
            TaskClass::LongTask => TaskBudget::long_task(),
            TaskClass::FastQa
            | TaskClass::ReadOnly
            | TaskClass::Write
            | TaskClass::VehicleAction => TaskBudget::fast_qa(),
        };
        TaskBudget {
            max_memory_bytes: base.max_memory_bytes.min(self.max_task_memory_bytes),
            max_storage_bytes: base.max_storage_bytes.min(self.max_task_storage_bytes),
            ..base
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_profile_constrains_long_task_budget() {
        let budget = DeviceProfile::entry().budget_for(TaskClass::LongTask);

        assert_eq!(budget.max_memory_bytes, 32 * 1024 * 1024);
        assert_eq!(budget.max_storage_bytes, 4 * 1024 * 1024);
    }

    #[test]
    fn flagship_keeps_long_task_budget() {
        let budget = DeviceProfile::flagship().budget_for(TaskClass::LongTask);

        assert_eq!(budget.max_memory_bytes, 128 * 1024 * 1024);
        assert_eq!(budget.max_storage_bytes, 16 * 1024 * 1024);
    }
}

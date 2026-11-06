//! Model gateway contracts and deterministic mock runtimes.
//!
//! Real model runtimes live behind the [`ModelRuntime`] boundary. They must
//! consume a bounded prompt and return either a structured tool action or a
//! completed response. Natural-language text is never treated as executable.

use std::fmt;

use harness_policy::ExecutionMode;
use harness_tools::ToolAction;

/// Error returned by a model runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    /// No model runtime is currently available.
    Unavailable,
    /// The model output did not conform to the requested schema.
    InvalidOutput,
    /// The requested generation exceeded the model budget.
    BudgetExceeded,
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => write!(f, "model runtime unavailable"),
            Self::InvalidOutput => write!(f, "model output invalid"),
            Self::BudgetExceeded => write!(f, "model budget exceeded"),
        }
    }
}

impl std::error::Error for ModelError {}

/// Policy controlling optional cloud model execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloudFallbackPolicy {
    /// Whether cloud execution is enabled at all.
    pub enabled: bool,
    /// Whether explicit user consent is required.
    pub require_user_consent: bool,
    /// Whether cloud is allowed while driving.
    pub allow_driving: bool,
    /// Whether privacy filtering must be confirmed before cloud transport.
    pub privacy_filter_required: bool,
}

impl CloudFallbackPolicy {
    /// Creates a fully disabled cloud policy.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            enabled: false,
            require_user_consent: true,
            allow_driving: false,
            privacy_filter_required: true,
        }
    }

    /// Decides whether local, cloud, or neither execution is allowed.
    #[must_use]
    pub fn decide(
        &self,
        user_consent: bool,
        mode: ExecutionMode,
        privacy_sensitive: bool,
        privacy_filter_confirmed: bool,
    ) -> CloudDecision {
        if !self.enabled {
            return CloudDecision::Rejected(CloudRejection::Disabled);
        }
        if self.require_user_consent && !user_consent {
            return CloudDecision::Rejected(CloudRejection::MissingConsent);
        }
        if !self.allow_driving
            && matches!(mode, ExecutionMode::DriveHighSpeed | ExecutionMode::Reverse)
        {
            return CloudDecision::Rejected(CloudRejection::DrivingDisallowed);
        }
        if privacy_sensitive && self.privacy_filter_required && !privacy_filter_confirmed {
            return CloudDecision::Rejected(CloudRejection::PrivacySensitive);
        }
        CloudDecision::Cloud
    }
}

/// Result of cloud fallback policy evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudDecision {
    /// Use the local model path.
    Local,
    /// Use the cloud model path.
    Cloud,
    /// Cloud execution was rejected.
    Rejected(CloudRejection),
}

/// Reason cloud execution was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudRejection {
    /// Cloud is disabled.
    Disabled,
    /// User consent is missing.
    MissingConsent,
    /// Cloud is not allowed in the current driving mode.
    DrivingDisallowed,
    /// Privacy-sensitive input has not been confirmed for redaction.
    PrivacySensitive,
}

/// Runtime-level request passed to the model router.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouterRequest {
    /// Whether the user explicitly consented to cloud execution.
    pub user_consent: bool,
    /// Current execution mode.
    pub mode: ExecutionMode,
    /// Whether the input is privacy-sensitive.
    pub privacy_sensitive: bool,
    /// Whether redaction was confirmed before transport.
    pub privacy_filter_confirmed: bool,
}

/// Routes each model call between local and consent-gated cloud runtimes.
#[derive(Debug)]
pub struct ModelRouter<L, C>
where
    L: ModelRuntime,
    C: ModelRuntime,
{
    local: L,
    cloud: C,
    policy: CloudFallbackPolicy,
}

impl<L, C> ModelRouter<L, C>
where
    L: ModelRuntime,
    C: ModelRuntime,
{
    /// Creates a router with local and cloud runtimes.
    #[must_use]
    pub fn new(local: L, cloud: C, policy: CloudFallbackPolicy) -> Self {
        Self {
            local,
            cloud,
            policy,
        }
    }

    /// Generates one bounded model response using local or cloud path.
    pub fn generate(
        &mut self,
        prompt: ModelPrompt<'_>,
        request: RouterRequest,
    ) -> Result<ModelResponse, ModelError> {
        match self.policy.decide(
            request.user_consent,
            request.mode,
            request.privacy_sensitive,
            request.privacy_filter_confirmed,
        ) {
            CloudDecision::Cloud => self.cloud.generate(prompt),
            CloudDecision::Local | CloudDecision::Rejected(_) => self.local.generate(prompt),
        }
    }
}

/// Bounded prompt supplied to a model runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelPrompt<'a> {
    /// Current user goal.
    pub goal: &'a str,
    /// Maximum tokens available to this call.
    pub max_tokens: u64,
}

/// Structured output returned by a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelOutput {
    /// A tool action awaiting deterministic policy validation.
    Action(ToolAction),
    /// A completed answer; no executable action is implied.
    Completed(String),
}

/// A model response with explicit token accounting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelResponse {
    /// Structured model output.
    pub output: ModelOutput,
    /// Tokens consumed by the response.
    pub tokens: u64,
}

/// Boundary implemented by local and cloud model runtimes.
pub trait ModelRuntime {
    /// Generates one bounded, structured model response.
    fn generate(&mut self, prompt: ModelPrompt<'_>) -> Result<ModelResponse, ModelError>;
}

/// Deterministic runtime used for tests, replay, and offline simulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockModelRuntime {
    output: ModelOutput,
    tokens: u64,
    unavailable: bool,
}

impl MockModelRuntime {
    /// Creates a runtime that always returns the supplied structured output.
    #[must_use]
    pub fn new(output: ModelOutput, tokens: u64) -> Self {
        Self {
            output,
            tokens,
            unavailable: false,
        }
    }

    /// Marks the runtime as unavailable.
    #[must_use]
    pub fn unavailable(mut self) -> Self {
        self.unavailable = true;
        self
    }
}

impl ModelRuntime for MockModelRuntime {
    fn generate(&mut self, prompt: ModelPrompt<'_>) -> Result<ModelResponse, ModelError> {
        if self.unavailable {
            return Err(ModelError::Unavailable);
        }
        if self.tokens > prompt.max_tokens {
            return Err(ModelError::BudgetExceeded);
        }
        if prompt.goal.trim().is_empty() {
            return Err(ModelError::InvalidOutput);
        }
        Ok(ModelResponse {
            output: self.output.clone(),
            tokens: self.tokens,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_policy::ExecutionMode;
    use harness_tools::ToolAction;

    #[test]
    fn mock_runtime_respects_token_budget() {
        let mut runtime =
            MockModelRuntime::new(ModelOutput::Action(ToolAction::read_local("status")), 101);
        let prompt = ModelPrompt {
            goal: "read status",
            max_tokens: 100,
        };

        assert_eq!(runtime.generate(prompt), Err(ModelError::BudgetExceeded));
    }

    #[test]
    fn cloud_fallback_requires_consent_and_privacy_confirmation() {
        let policy = CloudFallbackPolicy {
            enabled: true,
            require_user_consent: true,
            allow_driving: false,
            privacy_filter_required: true,
        };

        assert_eq!(
            policy.decide(false, ExecutionMode::Park, true, false),
            CloudDecision::Rejected(CloudRejection::MissingConsent)
        );
        assert_eq!(
            policy.decide(true, ExecutionMode::Park, true, false),
            CloudDecision::Rejected(CloudRejection::PrivacySensitive)
        );
        assert_eq!(
            policy.decide(true, ExecutionMode::Park, false, false),
            CloudDecision::Cloud
        );
        assert_eq!(
            policy.decide(true, ExecutionMode::DriveHighSpeed, false, false),
            CloudDecision::Rejected(CloudRejection::DrivingDisallowed)
        );
    }

    #[test]
    fn model_router_routes_local_and_cloud() {
        let local = MockModelRuntime::new(ModelOutput::Completed("local".into()), 10);
        let cloud = MockModelRuntime::new(ModelOutput::Completed("cloud".into()), 10);
        let policy = CloudFallbackPolicy {
            enabled: true,
            require_user_consent: true,
            allow_driving: false,
            privacy_filter_required: true,
        };
        let mut router = ModelRouter::new(local, cloud, policy);
        let prompt = ModelPrompt {
            goal: "route",
            max_tokens: 100,
        };

        let local_out = router
            .generate(
                prompt,
                RouterRequest {
                    user_consent: false,
                    mode: ExecutionMode::Park,
                    privacy_sensitive: false,
                    privacy_filter_confirmed: false,
                },
            )
            .expect("local path");
        assert_eq!(local_out.output, ModelOutput::Completed("local".into()));

        let cloud_out = router
            .generate(
                prompt,
                RouterRequest {
                    user_consent: true,
                    mode: ExecutionMode::Park,
                    privacy_sensitive: false,
                    privacy_filter_confirmed: false,
                },
            )
            .expect("cloud path");
        assert_eq!(cloud_out.output, ModelOutput::Completed("cloud".into()));
    }
}

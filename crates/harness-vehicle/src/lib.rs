//! Deterministic vehicle API gateway and Android Automotive VHAL read path.
//!
//! The gateway never executes vehicle actions. It produces a decision that a
//! platform adapter (VHAL, D-Bus, SOME/IP, vendor) then applies.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use harness_policy::{Capability, ExecutionMode, RiskLevel};

/// Classification of a vehicle API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VehicleApiKind {
    /// Read-only vehicle state.
    ReadOnly,
    /// Write/action vehicle API.
    Write,
}

/// Per-API rate limit with a fixed window in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimit {
    /// Maximum calls inside the window.
    pub max_calls: u32,
    /// Window size in seconds.
    pub window_seconds: u64,
}

/// Manifest for one whitelisted vehicle API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VehicleApi {
    /// Stable API name.
    pub name: &'static str,
    /// Read-only or write classification.
    pub kind: VehicleApiKind,
    /// Maximum risk assigned to this API.
    pub risk: RiskLevel,
    /// Capabilities required to call this API.
    pub required_capabilities: &'static [Capability],
    /// Whether the API may be used while driving.
    pub driving_allowed: bool,
    /// Whether approval is always required.
    pub requires_approval: bool,
    /// Rate limit for this API.
    pub rate_limit: RateLimit,
}

impl VehicleApi {
    /// Creates a read-only vehicle API manifest.
    #[must_use]
    pub const fn read_only(name: &'static str, max_calls: u32, window_seconds: u64) -> Self {
        Self {
            name,
            kind: VehicleApiKind::ReadOnly,
            risk: RiskLevel::Low,
            required_capabilities: &[Capability::VehicleRead],
            driving_allowed: true,
            requires_approval: false,
            rate_limit: RateLimit {
                max_calls,
                window_seconds,
            },
        }
    }

    /// Creates a write/action vehicle API manifest.
    #[must_use]
    pub const fn write(name: &'static str, max_calls: u32, window_seconds: u64) -> Self {
        Self {
            name,
            kind: VehicleApiKind::Write,
            risk: RiskLevel::Medium,
            required_capabilities: &[Capability::VehicleWrite],
            driving_allowed: false,
            requires_approval: true,
            rate_limit: RateLimit {
                max_calls,
                window_seconds,
            },
        }
    }
}

/// Registry of allowed vehicle APIs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VehicleApiRegistry {
    manifests: BTreeMap<&'static str, VehicleApi>,
}

impl VehicleApiRegistry {
    /// Creates the Phase 0 whitelist.
    #[must_use]
    pub fn phase0() -> Self {
        let mut registry = Self::default();
        for api in [
            VehicleApi::read_only("vehicle.speed", 20, 60),
            VehicleApi::read_only("vehicle.gear", 20, 60),
            VehicleApi::read_only("vehicle.battery_level", 20, 60),
            VehicleApi::read_only("vehicle.charging_status", 20, 60),
            VehicleApi::read_only("cabin.temperature", 20, 60),
            VehicleApi::write("climate.set_temperature", 2, 60),
        ] {
            registry.register(api);
        }
        registry
    }

    /// Registers or replaces a vehicle API manifest.
    pub fn register(&mut self, api: VehicleApi) {
        self.manifests.insert(api.name, api);
    }

    /// Returns a manifest by API name.
    #[must_use]
    pub fn manifest(&self, name: &str) -> Option<&VehicleApi> {
        self.manifests.get(name)
    }
}

/// Reason a vehicle API call was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VehicleApiReason {
    /// API is not registered.
    UnknownApi,
    /// API is forbidden.
    Forbidden,
    /// Driving mode does not allow vehicle API calls.
    ModeDisallowed,
    /// API cannot run while driving.
    DrivingDisallowed,
    /// Required capability is missing.
    MissingCapability(Capability),
    /// Rate limit reached.
    RateLimited,
}

impl fmt::Display for VehicleApiReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownApi => write!(f, "unknown vehicle api"),
            Self::Forbidden => write!(f, "forbidden vehicle api"),
            Self::ModeDisallowed => write!(f, "execution mode disallowed"),
            Self::DrivingDisallowed => write!(f, "vehicle api disallowed while driving"),
            Self::MissingCapability(capability) => {
                write!(f, "missing capability: {capability:?}")
            }
            Self::RateLimited => write!(f, "vehicle api rate limit reached"),
        }
    }
}

/// Deterministic decision produced by the vehicle API gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VehicleApiDecision {
    /// Whether the call may proceed.
    pub allowed: bool,
    /// Whether user approval is required before the call.
    pub requires_approval: bool,
    /// Classification of the requested API.
    pub kind: VehicleApiKind,
    /// Reasons explaining the decision.
    pub reasons: Vec<VehicleApiReason>,
}

/// Deterministic vehicle API gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VehicleApiGateway {
    registry: VehicleApiRegistry,
    calls: BTreeMap<&'static str, Vec<u64>>,
}

impl VehicleApiGateway {
    /// Creates a gateway from a registry.
    #[must_use]
    pub fn new(registry: VehicleApiRegistry) -> Self {
        Self {
            registry,
            calls: BTreeMap::new(),
        }
    }

    /// Evaluates one vehicle API call.
    ///
    /// Approved calls are recorded against the API rate limit.
    pub fn evaluate(
        &mut self,
        api_name: &str,
        mode: ExecutionMode,
        granted: &BTreeSet<Capability>,
        now_seconds: u64,
    ) -> VehicleApiDecision {
        let Some(api) = self.registry.manifest(api_name).cloned() else {
            return VehicleApiDecision {
                allowed: false,
                requires_approval: false,
                kind: VehicleApiKind::ReadOnly,
                reasons: vec![VehicleApiReason::UnknownApi],
            };
        };
        let mut reasons = Vec::new();

        if api.risk == RiskLevel::Forbidden {
            reasons.push(VehicleApiReason::Forbidden);
        }
        if matches!(mode, ExecutionMode::Reverse | ExecutionMode::SystemBusy) {
            reasons.push(VehicleApiReason::ModeDisallowed);
        }
        if !api.driving_allowed && !matches!(mode, ExecutionMode::Park | ExecutionMode::Charging) {
            reasons.push(VehicleApiReason::DrivingDisallowed);
        }
        for capability in api.required_capabilities {
            if !granted.contains(capability) {
                reasons.push(VehicleApiReason::MissingCapability(*capability));
            }
        }

        if reasons.is_empty() && self.rate_limited(&api, now_seconds) {
            reasons.push(VehicleApiReason::RateLimited);
        }

        VehicleApiDecision {
            allowed: reasons.is_empty(),
            requires_approval: api.requires_approval,
            kind: api.kind,
            reasons,
        }
    }

    fn rate_limited(&mut self, api: &VehicleApi, now_seconds: u64) -> bool {
        let cutoff = now_seconds.saturating_sub(api.rate_limit.window_seconds);
        let calls = self.calls.entry(api.name).or_default();
        calls.retain(|timestamp| *timestamp > cutoff);
        if calls.len() >= api.rate_limit.max_calls as usize {
            return true;
        }
        calls.push(now_seconds);
        false
    }
}

/// VHAL property read request produced by the read gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhalReadRequest {
    /// Stable property name mapped to Android VHAL by the platform adapter.
    pub property_name: String,
    /// Android VHAL area id.
    pub area_id: i32,
}

/// VHAL property read result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhalReadResult {
    /// Raw, already-redacted value returned by the platform adapter.
    pub value: String,
}

/// Error returned by the VHAL read gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VhalReadError {
    /// API is not registered.
    UnknownApi,
    /// API is forbidden.
    Forbidden,
    /// Mode disallows reads.
    ModeDisallowed,
    /// API cannot run while driving.
    DrivingDisallowed,
    /// Required capability is missing.
    MissingCapability(Capability),
    /// Rate limit reached.
    RateLimited,
    /// Write API reached a read-only gateway.
    WriteApiDenied,
    /// Approval is required before the call.
    ApprovalRequired,
    /// Platform reader failed.
    Reader(String),
}

impl fmt::Display for VhalReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownApi => write!(f, "unknown vehicle api"),
            Self::Forbidden => write!(f, "forbidden vehicle api"),
            Self::ModeDisallowed => write!(f, "mode disallowed"),
            Self::DrivingDisallowed => write!(f, "driving disallowed"),
            Self::MissingCapability(capability) => {
                write!(f, "missing capability: {capability:?}")
            }
            Self::RateLimited => write!(f, "rate limited"),
            Self::WriteApiDenied => write!(f, "write api denied"),
            Self::ApprovalRequired => write!(f, "approval required"),
            Self::Reader(message) => write!(f, "reader error: {message}"),
        }
    }
}

impl std::error::Error for VhalReadError {}

/// Platform vehicle property reader implemented by Android Automotive adapters.
pub trait VehiclePropertyReader {
    /// Reads one vehicle property.
    fn read(&mut self, request: VhalReadRequest) -> Result<VhalReadResult, String>;
}

/// Rust-side Android Automotive VHAL read-only gateway.
#[derive(Debug)]
pub struct VhalReadGateway<R>
where
    R: VehiclePropertyReader,
{
    api_gateway: VehicleApiGateway,
    reader: R,
}

impl<R> VhalReadGateway<R>
where
    R: VehiclePropertyReader,
{
    /// Creates a read-only VHAL gateway.
    #[must_use]
    pub fn new(registry: VehicleApiRegistry, reader: R) -> Self {
        Self {
            api_gateway: VehicleApiGateway::new(registry),
            reader,
        }
    }

    /// Reads one whitelisted VHAL property after policy evaluation.
    pub fn read(
        &mut self,
        api_name: &str,
        area_id: i32,
        mode: ExecutionMode,
        granted: &BTreeSet<Capability>,
        now_seconds: u64,
    ) -> Result<VhalReadResult, VhalReadError> {
        let decision = self
            .api_gateway
            .evaluate(api_name, mode, granted, now_seconds);
        if !decision.allowed {
            return Err(reason_to_read_error(decision.reasons.first()));
        }
        if decision.kind == VehicleApiKind::Write {
            return Err(VhalReadError::WriteApiDenied);
        }
        if decision.requires_approval {
            return Err(VhalReadError::ApprovalRequired);
        }

        self.reader
            .read(VhalReadRequest {
                property_name: api_name.to_owned(),
                area_id,
            })
            .map_err(VhalReadError::Reader)
    }
}

fn reason_to_read_error(reason: Option<&VehicleApiReason>) -> VhalReadError {
    match reason {
        None | Some(VehicleApiReason::UnknownApi) => VhalReadError::UnknownApi,
        Some(VehicleApiReason::Forbidden) => VhalReadError::Forbidden,
        Some(VehicleApiReason::ModeDisallowed) => VhalReadError::ModeDisallowed,
        Some(VehicleApiReason::DrivingDisallowed) => VhalReadError::DrivingDisallowed,
        Some(VehicleApiReason::MissingCapability(capability)) => {
            VhalReadError::MissingCapability(*capability)
        }
        Some(VehicleApiReason::RateLimited) => VhalReadError::RateLimited,
    }
}

/// Deterministic mock VHAL property reader for tests and simulation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MockVehiclePropertyReader {
    values: BTreeMap<String, String>,
}

impl MockVehiclePropertyReader {
    /// Seeds one property value.
    pub fn seed(&mut self, property_name: &str, value: &str) {
        self.values
            .insert(property_name.to_owned(), value.to_owned());
    }
}

impl VehiclePropertyReader for MockVehiclePropertyReader {
    fn read(&mut self, request: VhalReadRequest) -> Result<VhalReadResult, String> {
        self.values
            .get(&request.property_name)
            .cloned()
            .map(|value| VhalReadResult { value })
            .ok_or_else(|| format!("unknown property: {}", request.property_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_capabilities() -> BTreeSet<Capability> {
        BTreeSet::from([Capability::VehicleRead])
    }

    #[test]
    fn read_gateway_returns_whitelisted_property() {
        let mut reader = MockVehiclePropertyReader::default();
        reader.seed("vehicle.speed", "42.0");
        let mut gateway = VhalReadGateway::new(VehicleApiRegistry::phase0(), reader);

        let result = gateway
            .read(
                "vehicle.speed",
                0,
                ExecutionMode::DriveHighSpeed,
                &read_capabilities(),
                1,
            )
            .expect("speed read should succeed");

        assert_eq!(result.value, "42.0");
    }

    #[test]
    fn read_gateway_rejects_unknown_api() {
        let mut gateway = VhalReadGateway::new(
            VehicleApiRegistry::phase0(),
            MockVehiclePropertyReader::default(),
        );

        assert_eq!(
            gateway.read(
                "vehicle.unknown",
                0,
                ExecutionMode::Park,
                &read_capabilities(),
                1,
            ),
            Err(VhalReadError::UnknownApi)
        );
    }

    #[test]
    fn read_gateway_rejects_write_api() {
        let mut gateway = VhalReadGateway::new(
            VehicleApiRegistry::phase0(),
            MockVehiclePropertyReader::default(),
        );

        assert_eq!(
            gateway.read(
                "climate.set_temperature",
                0,
                ExecutionMode::Park,
                &BTreeSet::from([Capability::VehicleWrite]),
                1,
            ),
            Err(VhalReadError::WriteApiDenied)
        );
    }

    #[test]
    fn read_gateway_rejects_reverse_mode() {
        let mut gateway = VhalReadGateway::new(
            VehicleApiRegistry::phase0(),
            MockVehiclePropertyReader::default(),
        );

        assert_eq!(
            gateway.read(
                "vehicle.speed",
                0,
                ExecutionMode::Reverse,
                &read_capabilities(),
                1,
            ),
            Err(VhalReadError::ModeDisallowed)
        );
    }

    #[test]
    fn read_gateway_enforces_rate_limit() {
        let mut reader = MockVehiclePropertyReader::default();
        reader.seed("vehicle.gear", "park");
        let mut registry = VehicleApiRegistry::default();
        registry.register(VehicleApi::read_only("vehicle.gear", 2, 60));
        let mut gateway = VhalReadGateway::new(registry, reader);

        for second in 1..=2 {
            gateway
                .read(
                    "vehicle.gear",
                    0,
                    ExecutionMode::Park,
                    &read_capabilities(),
                    second,
                )
                .expect("within rate limit");
        }
        assert_eq!(
            gateway.read(
                "vehicle.gear",
                0,
                ExecutionMode::Park,
                &read_capabilities(),
                3,
            ),
            Err(VhalReadError::RateLimited)
        );
    }

    #[test]
    fn read_gateway_requires_vehicle_capability() {
        let mut gateway = VhalReadGateway::new(
            VehicleApiRegistry::phase0(),
            MockVehiclePropertyReader::default(),
        );

        assert_eq!(
            gateway.read("vehicle.speed", 0, ExecutionMode::Park, &BTreeSet::new(), 1,),
            Err(VhalReadError::MissingCapability(Capability::VehicleRead))
        );
    }
}

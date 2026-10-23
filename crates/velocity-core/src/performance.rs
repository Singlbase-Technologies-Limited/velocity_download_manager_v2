//! Performance profiles (#57, ADR-0005): limits for the adaptive engine, not aria2 option sets.
//! Each profile sets the most connections a download may use to one server and the smallest
//! piece it is split into; within them the engine adds connections one at a time, only while
//! each helps (#47). All profiles add a connection at the same 10% gain until #77 measures
//! whether other thresholds help (PO decision on #57).
//!
//! | Profile | Connections per server | Smallest piece |
//! |---|---|---|
//! | Conservative | 2 | 20 MiB (V1's) |
//! | Balanced, the default | 8 | 1 MiB: today's engine values |
//! | Maximum | 16 (V1's) | 1 MiB |
//! | Custom | the user's | the user's |

use serde::{Deserialize, Serialize};

use crate::domain::ErrorCode;
use crate::transfer::EngineLimits;

/// A performance profile (V1's names).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PerformanceProfile {
    Conservative,
    #[default]
    Balanced,
    Maximum,
    Custom,
}

/// The Custom profile's own limits (ADR-0005: V1's `max_connection_per_server` and
/// `min_split_size_mib`; aria2's `split` is gone, as V2 splits adaptively).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomProfile {
    pub max_connections_per_server: u32,
    pub min_segment_mib: u32,
}

impl Default for CustomProfile {
    /// Balanced's values.
    fn default() -> Self {
        CustomProfile {
            max_connections_per_server: 8,
            min_segment_mib: 1,
        }
    }
}

/// The performance settings: a profile, and the Custom profile's limits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PerformanceSettings {
    pub profile: PerformanceProfile,
    pub custom: CustomProfile,
}

impl PerformanceSettings {
    /// The engine limits the profile sets.
    pub fn engine_limits(&self) -> EngineLimits {
        const MIB: u64 = 1 << 20;
        let (max_connections_per_host, min_segment_bytes) = match self.profile {
            PerformanceProfile::Conservative => (2, 20 * MIB),
            PerformanceProfile::Balanced => (
                EngineLimits::DEFAULT.max_connections_per_host,
                EngineLimits::DEFAULT.min_segment_bytes,
            ),
            PerformanceProfile::Maximum => (EngineLimits::MAX_CONNECTIONS_PER_HOST, MIB),
            PerformanceProfile::Custom => (
                self.custom.max_connections_per_server,
                u64::from(self.custom.min_segment_mib) * MIB,
            ),
        };
        EngineLimits {
            max_connections_per_host,
            min_segment_bytes,
        }
    }

    /// `InvalidInput` when the Custom limits are out of bounds (1 to 16 per server, pieces of 1 to
    /// 1024 MiB), checked whichever profile is chosen, so switching to Custom never fails later.
    pub fn check(&self) -> Result<(), ErrorCode> {
        PerformanceSettings {
            profile: PerformanceProfile::Custom,
            ..*self
        }
        .engine_limits()
        .check()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1 << 20;

    fn limits(profile: PerformanceProfile) -> (u32, u64) {
        let l = PerformanceSettings {
            profile,
            custom: CustomProfile {
                max_connections_per_server: 5,
                min_segment_mib: 3,
            },
        }
        .engine_limits();
        (l.max_connections_per_host, l.min_segment_bytes)
    }

    #[test]
    fn each_profile_maps_to_its_engine_limits() {
        use PerformanceProfile::*;
        assert_eq!(limits(Conservative), (2, 20 * MIB));
        assert_eq!(limits(Balanced), (8, MIB));
        assert_eq!(limits(Maximum), (16, MIB));
        assert_eq!(limits(Custom), (5, 3 * MIB));
    }

    #[test]
    fn balanced_is_the_default_and_todays_engine_values() {
        let s = PerformanceSettings::default();
        assert_eq!(s.profile, PerformanceProfile::Balanced);
        assert_eq!(s.engine_limits(), EngineLimits::DEFAULT);
        assert_eq!(s.check(), Ok(()));
        // Every profile's limits are ones an engine accepts.
        for profile in [
            PerformanceProfile::Conservative,
            PerformanceProfile::Balanced,
            PerformanceProfile::Maximum,
            PerformanceProfile::Custom,
        ] {
            let l = PerformanceSettings { profile, ..s }.engine_limits();
            assert_eq!(l.check(), Ok(()), "{profile:?}");
        }
    }

    #[test]
    fn custom_limits_out_of_bounds_are_refused() {
        let custom = |max_connections_per_server, min_segment_mib| PerformanceSettings {
            profile: PerformanceProfile::Balanced,
            custom: CustomProfile {
                max_connections_per_server,
                min_segment_mib,
            },
        };
        assert_eq!(custom(16, 1024).check(), Ok(()));
        for bad in [custom(0, 8), custom(17, 8), custom(8, 0), custom(8, 1025)] {
            assert_eq!(bad.check(), Err(ErrorCode::InvalidInput), "{bad:?}");
        }
    }
}

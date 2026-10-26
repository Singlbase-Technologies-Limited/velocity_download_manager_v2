//! Router port mapping (UPnP and NAT-PMP) as the user sees it (V1's `portmap.rs`, #62).
//!
//! libtorrent tries both protocols when mapping is on and reports each result as an alert. When
//! no router answers, UPnP usually reports nothing at all, so a search with no result after
//! [`PORT_MAP_SEARCH_SECS`] shows as "no router found" rather than searching forever.

use std::time::{Duration, Instant};

use velocity_core::torrent_engine::{PortMapState, PORT_MAP_SEARCH_SECS};

const SEARCH_TIMEOUT: Duration = Duration::from_secs(PORT_MAP_SEARCH_SECS);

#[derive(Debug, Clone, Default, PartialEq)]
enum Transport {
    #[default]
    Waiting,
    Mapped(u16),
    Failed(String),
}

/// What libtorrent said about each protocol since mapping was last turned on.
#[derive(Debug, Clone, Default)]
pub struct PortMapTracker {
    enabled: bool,
    since: Option<Instant>,
    upnp: Transport,
    natpmp: Transport,
}

impl PortMapTracker {
    /// Turning mapping on, or changing the port, starts a new search.
    pub fn reset(&mut self, enabled: bool, now: Instant) {
        *self = PortMapTracker {
            enabled,
            since: enabled.then_some(now),
            ..Default::default()
        };
    }

    fn slot(&mut self, transport: &str) -> Option<&mut Transport> {
        if !self.enabled {
            return None;
        }
        match transport {
            "upnp" => Some(&mut self.upnp),
            "natpmp" => Some(&mut self.natpmp),
            _ => None,
        }
    }

    pub fn on_mapped(&mut self, transport: &str, external_port: u16) {
        if let Some(t) = self.slot(transport) {
            *t = Transport::Mapped(external_port);
        }
    }

    pub fn on_error(&mut self, transport: &str, message: &str) {
        if let Some(t) = self.slot(transport) {
            // A later error never hides a working mapping.
            if !matches!(t, Transport::Mapped(_)) {
                *t = Transport::Failed(message.to_owned());
            }
        }
    }

    pub fn state(&self, now: Instant) -> PortMapState {
        if !self.enabled {
            return PortMapState::Off {};
        }
        for (t, via) in [(&self.upnp, "upnp"), (&self.natpmp, "natpmp")] {
            if let Transport::Mapped(port) = t {
                return PortMapState::Mapped {
                    external_port: *port,
                    via: via.into(),
                };
            }
        }
        if let (Transport::Failed(u), Transport::Failed(n)) = (&self.upnp, &self.natpmp) {
            return PortMapState::Failed {
                message: format!("UPnP: {u}; NAT-PMP: {n}"),
            };
        }
        let waited = self
            .since
            .map_or(Duration::ZERO, |t| now.saturating_duration_since(t));
        if waited >= SEARCH_TIMEOUT {
            PortMapState::NoRouter {}
        } else {
            PortMapState::Searching {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// V1's `off_until_the_user_turns_it_on`.
    #[test]
    fn off_until_the_user_turns_it_on() {
        let t0 = Instant::now();
        let mut p = PortMapTracker::default();
        assert_eq!(p.state(t0), PortMapState::Off {});
        p.reset(false, t0);
        // Results that arrive while off (a late alert) change nothing.
        p.on_mapped("upnp", 6890);
        assert_eq!(p.state(t0), PortMapState::Off {});
    }

    /// V1's `a_mapping_from_either_protocol_wins`.
    #[test]
    fn a_mapping_from_either_protocol_wins() {
        let t0 = Instant::now();
        let mut p = PortMapTracker::default();
        p.reset(true, t0);
        assert_eq!(p.state(t0), PortMapState::Searching {});
        p.on_error("natpmp", "no router");
        assert_eq!(
            p.state(t0),
            PortMapState::Searching {},
            "UPnP may still answer"
        );
        p.on_mapped("upnp", 40000);
        let mapped = PortMapState::Mapped {
            external_port: 40000,
            via: "upnp".into(),
        };
        assert_eq!(p.state(t0), mapped);
        // Still mapped long after the search window.
        assert_eq!(p.state(t0 + SEARCH_TIMEOUT * 10), mapped);
    }

    /// V1's `both_failing_is_a_failure_with_the_reasons`.
    #[test]
    fn both_failing_is_a_failure_with_the_reasons() {
        let t0 = Instant::now();
        let mut p = PortMapTracker::default();
        p.reset(true, t0);
        p.on_error("upnp", "conflict in mapping");
        p.on_error("natpmp", "not authorized");
        match p.state(t0) {
            PortMapState::Failed { message } => {
                assert!(message.contains("UPnP: conflict in mapping"), "{message}");
                assert!(message.contains("NAT-PMP: not authorized"), "{message}");
            }
            s => panic!("{s:?}"),
        }
    }

    /// V1's `silence_means_no_router_and_a_reset_searches_again`.
    #[test]
    fn silence_means_no_router_and_a_reset_searches_again() {
        let t0 = Instant::now();
        let mut p = PortMapTracker::default();
        p.reset(true, t0);
        assert_eq!(p.state(t0 + SEARCH_TIMEOUT / 2), PortMapState::Searching {});
        assert_eq!(p.state(t0 + SEARCH_TIMEOUT), PortMapState::NoRouter {});
        // One protocol failed and the other never answered: still "no router", with nothing
        // mapped.
        p.on_error("natpmp", "timed out");
        assert_eq!(p.state(t0 + SEARCH_TIMEOUT), PortMapState::NoRouter {});
        let t1 = t0 + SEARCH_TIMEOUT * 2;
        p.reset(true, t1);
        assert_eq!(p.state(t1), PortMapState::Searching {});
        // A mapping that arrives late still counts.
        p.on_mapped("natpmp", 6890);
        assert!(matches!(
            p.state(t1 + SEARCH_TIMEOUT * 3),
            PortMapState::Mapped { .. }
        ));
        // Unknown transports are ignored.
        p.on_error("pcp?", "x");
        assert!(matches!(p.state(t1), PortMapState::Mapped { .. }));
    }
}

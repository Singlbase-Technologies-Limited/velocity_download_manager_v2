//! Shares connections between running downloads (#47, report section 5.5): each host gets at most
//! the per-host cap, all downloads together at most the shared budget, and within both the share
//! is max-min fair: no download gets more while another that wants more has fewer.

use std::collections::HashMap;

use velocity_core::domain::ConnectionLimit;

/// What one running download wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Demand<'a> {
    /// The host its connections go to, as the per-host cap counts them.
    pub host: &'a str,
    /// The connections it asks for, at least 1.
    pub wants: u32,
}

/// What one download is allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Share {
    pub allowed: u32,
    /// Why `allowed` is below what it wants; `None` when it isn't.
    pub limited_by: Option<ConnectionLimit>,
}

/// The shares of `demands`, in the same order. Connections are handed out one per download per
/// round, in order, so an uneven remainder goes to the earlier downloads.
///
/// Every download keeps one connection, even past the caps: a running download makes no progress
/// without one. How many downloads run at once is the queue's job (#50), and libcurl's own caps
/// queue anything past them (`CURLMOPT_MAX_HOST_CONNECTIONS`).
pub fn share(demands: &[Demand<'_>], per_host: u32, budget: u32) -> Vec<Share> {
    let mut allowed = vec![1u32; demands.len()];
    let mut by_host: HashMap<&str, u32> = HashMap::new();
    for d in demands {
        *by_host.entry(d.host).or_default() += 1;
    }
    let mut total = demands.len() as u32;
    loop {
        let mut gave = false;
        for (i, d) in demands.iter().enumerate() {
            let on_host = by_host.get_mut(d.host).expect("counted above");
            if allowed[i] < d.wants && *on_host < per_host && total < budget {
                allowed[i] += 1;
                *on_host += 1;
                total += 1;
                gave = true;
            }
        }
        if !gave {
            break;
        }
    }
    demands
        .iter()
        .zip(allowed)
        .map(|(d, allowed)| Share {
            allowed,
            limited_by: (allowed < d.wants).then(|| {
                if by_host[d.host] >= per_host {
                    ConnectionLimit::PerHostCap
                } else {
                    ConnectionLimit::SharedBudget
                }
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(demands: &[Demand<'_>], per_host: u32, budget: u32) -> Vec<u32> {
        share(demands, per_host, budget)
            .iter()
            .map(|s| s.allowed)
            .collect()
    }

    fn on(host: &str, wants: u32) -> Demand<'_> {
        Demand { host, wants }
    }

    #[test]
    fn a_download_alone_gets_what_it_wants() {
        assert_eq!(
            share(&[on("a", 4)], 8, 48),
            [Share {
                allowed: 4,
                limited_by: None
            }]
        );
    }

    #[test]
    fn downloads_from_one_host_split_its_cap() {
        let shares = share(&[on("a", 8), on("a", 8)], 8, 48);
        assert_eq!(shares.iter().map(|s| s.allowed).collect::<Vec<_>>(), [4, 4]);
        assert!(shares
            .iter()
            .all(|s| s.limited_by == Some(ConnectionLimit::PerHostCap)));
    }

    #[test]
    fn downloads_from_different_hosts_split_the_budget() {
        let shares = share(&[on("a", 8), on("b", 8)], 8, 10);
        assert_eq!(shares.iter().map(|s| s.allowed).collect::<Vec<_>>(), [5, 5]);
        assert!(shares
            .iter()
            .all(|s| s.limited_by == Some(ConnectionLimit::SharedBudget)));
    }

    #[test]
    fn what_a_small_download_leaves_goes_to_the_others() {
        // Max-min: the one that wants 2 gets 2, and the rest is split evenly.
        let shares = share(&[on("a", 2), on("b", 8), on("c", 8)], 8, 12);
        assert_eq!(
            shares.iter().map(|s| s.allowed).collect::<Vec<_>>(),
            [2, 5, 5]
        );
        assert_eq!(shares[0].limited_by, None);
    }

    #[test]
    fn an_uneven_remainder_goes_to_the_earlier_downloads() {
        assert_eq!(
            allowed(&[on("a", 8), on("b", 8), on("c", 8)], 8, 10),
            [4, 3, 3]
        );
    }

    #[test]
    fn the_host_cap_and_the_budget_apply_together() {
        // Host a's cap of 4 binds first; b takes what the budget of 7 has left, below its own
        // host's cap.
        let shares = share(&[on("a", 8), on("a", 8), on("b", 8)], 4, 7);
        assert_eq!(
            shares.iter().map(|s| s.allowed).collect::<Vec<_>>(),
            [2, 2, 3]
        );
        assert_eq!(shares[0].limited_by, Some(ConnectionLimit::PerHostCap));
        assert_eq!(shares[2].limited_by, Some(ConnectionLimit::SharedBudget));
    }

    #[test]
    fn every_download_keeps_one_connection_even_past_the_caps() {
        // A running download can't make progress without one; the queue (#50) bounds how many run.
        assert_eq!(
            allowed(&[on("a", 2), on("a", 2), on("a", 2)], 2, 2),
            [1, 1, 1]
        );
    }
}

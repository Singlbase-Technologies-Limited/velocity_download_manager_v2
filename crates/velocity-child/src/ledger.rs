//! What a supervisor's book keeps of a child's work (#39; shared since #62): the last state of each
//! item the running child has, the items earlier children had when they went away, and the
//! highest id any child assigned. The engine keeps transfers in one, the torrent helper torrents.

use std::collections::btree_map::Entry;
use std::collections::BTreeMap;

use velocity_core::domain::{DownloadStatus, FailureClass, TransferError};
use velocity_core::torrent_engine::{TorrentId, TorrentSnapshot};
use velocity_core::transfer::{TransferId, TransferSnapshot};

/// One item's state, as a child reports it.
pub trait Tracked: Clone {
    type Id: Ord + Copy;
    fn id(&self) -> Self::Id;
    /// The id as a number, so a new child can start after it.
    fn number(id: Self::Id) -> u64;
    /// The item won't change on its own any more.
    fn is_terminal(&self) -> bool;
    /// Marks an unfinished item as failed, with a retryable error, because its child went away.
    fn lose(&mut self, message: &str);
}

/// The items of the running child and of the children before it.
pub struct Ledger<S: Tracked> {
    current: BTreeMap<S::Id, S>,
    /// The items earlier children had when they stopped. All are finished; the supervisor answers
    /// for them until they are removed.
    orphans: BTreeMap<S::Id, S>,
    highest_id: u64,
}

impl<S: Tracked> Default for Ledger<S> {
    fn default() -> Self {
        Ledger {
            current: BTreeMap::new(),
            orphans: BTreeMap::new(),
            highest_id: 0,
        }
    }
}

impl<S: Tracked> Ledger<S> {
    pub fn highest_id(&self) -> u64 {
        self.highest_id
    }

    fn saw(&mut self, id: S::Id) {
        self.highest_id = self.highest_id.max(S::number(id));
    }

    /// A new child runs: it has no items yet.
    pub fn installed(&mut self) {
        self.current.clear();
    }

    /// The running child reported `snapshot`. An orphan it reports is its own again: the torrent
    /// helper's supervisor gives a new helper a crashed one's torrents under their ids (#62).
    pub fn changed(&mut self, snapshot: &S) {
        self.saw(snapshot.id());
        self.orphans.remove(&snapshot.id());
        self.current.insert(snapshot.id(), snapshot.clone());
    }

    /// The running child forgot `id`.
    pub fn removed(&mut self, id: S::Id) {
        self.current.remove(&id);
    }

    /// Records an item a child assigned, as `queued`, if no event has yet. `current` says whether
    /// that child still runs; if it doesn't, the item was lost between its id and this call, and
    /// the failed item is returned to pass on.
    pub fn added(&mut self, current: bool, mut queued: S, lost_message: &str) -> Option<S> {
        let id = queued.id();
        self.saw(id);
        if current {
            self.current.entry(id).or_insert(queued);
            None
        } else if let Entry::Vacant(slot) = self.orphans.entry(id) {
            queued.lose(lost_message);
            slot.insert(queued.clone());
            Some(queued)
        } else {
            None
        }
    }

    /// The running child went away: its unfinished items fail with `message` and become orphans.
    /// Returns the items that failed, to pass on.
    pub fn retired(&mut self, message: &str) -> Vec<S> {
        let mut lost = Vec::new();
        for (id, mut snapshot) in std::mem::take(&mut self.current) {
            if !snapshot.is_terminal() {
                snapshot.lose(message);
                lost.push(snapshot.clone());
            }
            self.orphans.insert(id, snapshot);
        }
        lost
    }

    pub fn is_orphan(&self, id: S::Id) -> bool {
        self.orphans.contains_key(&id)
    }

    /// Forgets an orphan; returns whether `id` was one.
    pub fn remove_orphan(&mut self, id: S::Id) -> bool {
        self.orphans.remove(&id).is_some()
    }

    pub fn orphans(&self) -> Vec<S> {
        self.orphans.values().cloned().collect()
    }
}

/// A transfer, in the engine's supervisor.
impl Tracked for TransferSnapshot {
    type Id = TransferId;
    fn id(&self) -> TransferId {
        self.id
    }
    fn number(id: TransferId) -> u64 {
        id.0
    }
    fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }
    fn lose(&mut self, message: &str) {
        self.status = DownloadStatus::Failed;
        self.progress.download_speed_bps = 0;
        self.progress.connections = 0;
        self.error = Some(TransferError::new(FailureClass::Transient, None, message));
    }
}

/// A torrent, in the torrent helper's supervisor.
impl Tracked for TorrentSnapshot {
    type Id = TorrentId;
    fn id(&self) -> TorrentId {
        self.id
    }
    fn number(id: TorrentId) -> u64 {
        id.0
    }
    fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }
    fn lose(&mut self, message: &str) {
        self.status = DownloadStatus::Failed;
        self.progress.download_speed_bps = 0;
        self.progress.upload_speed_bps = 0;
        self.progress.connections = 0;
        self.peers = 0;
        self.seeds = 0;
        self.error = Some(TransferError::new(FailureClass::Transient, None, message));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct Item {
        id: u64,
        done: bool,
        failed: Option<String>,
    }

    impl Tracked for Item {
        type Id = u64;
        fn id(&self) -> u64 {
            self.id
        }
        fn number(id: u64) -> u64 {
            id
        }
        fn is_terminal(&self) -> bool {
            self.done || self.failed.is_some()
        }
        fn lose(&mut self, message: &str) {
            self.failed = Some(message.to_owned());
        }
    }

    fn item(id: u64, done: bool) -> Item {
        Item {
            id,
            done,
            failed: None,
        }
    }

    #[test]
    fn a_retired_childs_unfinished_items_fail_and_stay_as_orphans() {
        let mut ledger = Ledger::default();
        ledger.changed(&item(1, false));
        ledger.changed(&item(2, true));
        let lost = ledger.retired("gone");
        assert_eq!(
            lost,
            [Item {
                failed: Some("gone".into()),
                ..item(1, false)
            }]
        );
        assert!(ledger.is_orphan(1) && ledger.is_orphan(2));
        assert_eq!(ledger.orphans().len(), 2);
        assert!(ledger.remove_orphan(2));
        assert!(!ledger.remove_orphan(2));
        assert_eq!(ledger.retired("again"), [], "nothing runs any more");
    }

    #[test]
    fn ids_only_grow_across_children() {
        let mut ledger = Ledger::default();
        ledger.changed(&item(7, false));
        ledger.retired("gone");
        ledger.installed();
        assert_eq!(ledger.highest_id(), 7);
        ledger.changed(&item(3, false));
        assert_eq!(ledger.highest_id(), 7);
        ledger.removed(3);
        assert_eq!(ledger.retired("x"), [], "a removed item isn't lost");
    }

    #[test]
    fn an_item_added_by_a_child_that_has_gone_is_lost_at_once() {
        let mut ledger = Ledger::default();
        assert_eq!(ledger.added(true, item(1, false), "lost"), None);
        let lost = ledger.added(false, item(2, false), "lost").unwrap();
        assert_eq!(lost.failed.as_deref(), Some("lost"));
        assert!(ledger.is_orphan(2));
        assert_eq!(
            ledger.added(false, item(2, false), "lost"),
            None,
            "only once"
        );
        assert_eq!(ledger.highest_id(), 2);
    }

    /// The torrent helper's supervisor adds a crashed helper's torrents to the next one under
    /// their own ids (#62): reported by the running child again, an orphan is its item again.
    #[test]
    fn an_orphan_the_running_child_reports_again_is_its_own_again() {
        let mut ledger = Ledger::default();
        ledger.changed(&item(1, false));
        ledger.changed(&item(2, false));
        ledger.retired("gone");
        ledger.installed();
        ledger.changed(&item(1, false));
        assert!(!ledger.is_orphan(1));
        assert!(ledger.is_orphan(2));
        assert_eq!(
            ledger.orphans(),
            [Item {
                failed: Some("gone".into()),
                ..item(2, false)
            }]
        );
        // Lost again with the next child.
        assert_eq!(ledger.retired("again").len(), 1);
        assert!(ledger.is_orphan(1));
    }
}

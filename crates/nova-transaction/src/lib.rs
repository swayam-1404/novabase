//! WAL-backed transaction states and strict collection-level locking.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use nova_core::error::{NovaError, Result};
use nova_wal::{TransactionId, WriteAheadLog};

/// Observable transaction lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionStatus {
    Active,
    Committed,
    RolledBack,
}

#[derive(Debug, Default)]
struct LockState {
    readers: BTreeSet<TransactionId>,
    writer: Option<TransactionId>,
}

#[derive(Debug)]
struct Inner {
    next_id: TransactionId,
    statuses: BTreeMap<TransactionId, TransactionStatus>,
    locks: BTreeMap<String, LockState>,
    wal: WriteAheadLog,
}

/// Thread-safe strict two-phase lock and transaction coordinator.
#[derive(Debug)]
pub struct TransactionManager {
    inner: Mutex<Inner>,
}

impl TransactionManager {
    /// Opens the transaction WAL and resumes ids above every recorded id.
    ///
    /// # Errors
    ///
    /// Returns a typed WAL I/O or corruption error.
    pub fn open(wal_path: impl AsRef<Path>) -> Result<Self> {
        let wal = WriteAheadLog::open(wal_path)?;
        let next_id = wal
            .records()
            .iter()
            .map(|record| record.transaction_id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| NovaError::InvalidState("transaction id overflow".to_owned()))?;
        Ok(Self {
            inner: Mutex::new(Inner {
                next_id,
                statuses: BTreeMap::new(),
                locks: BTreeMap::new(),
                wal,
            }),
        })
    }

    /// Begins a new active transaction and appends its WAL begin record.
    ///
    /// # Errors
    ///
    /// Returns a typed lock, WAL, or id-overflow error.
    pub fn begin(&self) -> Result<TransactionId> {
        let mut inner = self.lock()?;
        let id = inner.next_id;
        inner.next_id = inner
            .next_id
            .checked_add(1)
            .ok_or_else(|| NovaError::InvalidState("transaction id overflow".to_owned()))?;
        inner.wal.begin(id)?;
        inner.statuses.insert(id, TransactionStatus::Active);
        Ok(id)
    }

    /// Acquires or re-enters a shared collection lock.
    ///
    /// # Errors
    ///
    /// Returns a typed state error if the transaction is inactive or another
    /// transaction owns the exclusive lock.
    pub fn acquire_shared(&self, id: TransactionId, collection: &str) -> Result<()> {
        let mut inner = self.lock()?;
        ensure_active(&inner, id)?;
        let lock = inner.locks.entry(collection.to_owned()).or_default();
        if lock.writer.is_some_and(|writer| writer != id) {
            return conflict(collection);
        }
        lock.readers.insert(id);
        Ok(())
    }

    /// Acquires or upgrades to an exclusive collection lock.
    ///
    /// # Errors
    ///
    /// Returns a typed state error if another reader/writer conflicts.
    pub fn acquire_exclusive(&self, id: TransactionId, collection: &str) -> Result<()> {
        let mut inner = self.lock()?;
        ensure_active(&inner, id)?;
        let lock = inner.locks.entry(collection.to_owned()).or_default();
        let foreign_reader = lock.readers.iter().any(|reader| *reader != id);
        let foreign_writer = lock.writer.is_some_and(|writer| writer != id);
        if foreign_reader || foreign_writer {
            return conflict(collection);
        }
        lock.readers.remove(&id);
        lock.writer = Some(id);
        Ok(())
    }

    /// Commits durably, then releases every held lock.
    ///
    /// # Errors
    ///
    /// Returns a typed state, WAL append, or synchronization error.
    pub fn commit(&self, id: TransactionId) -> Result<()> {
        let mut inner = self.lock()?;
        ensure_active(&inner, id)?;
        inner.wal.commit(id)?;
        inner.wal.flush()?;
        inner.statuses.insert(id, TransactionStatus::Committed);
        release_locks(&mut inner.locks, id);
        Ok(())
    }

    /// Rolls back durably, then releases every held lock.
    ///
    /// # Errors
    ///
    /// Returns a typed state, WAL append, or synchronization error.
    pub fn rollback(&self, id: TransactionId) -> Result<()> {
        let mut inner = self.lock()?;
        ensure_active(&inner, id)?;
        inner.wal.abort(id)?;
        inner.wal.flush()?;
        inner.statuses.insert(id, TransactionStatus::RolledBack);
        release_locks(&mut inner.locks, id);
        Ok(())
    }

    /// Returns a known transaction's state.
    ///
    /// # Errors
    ///
    /// Returns [`NovaError::NotFound`] for an unknown id.
    pub fn status(&self, id: TransactionId) -> Result<TransactionStatus> {
        self.lock()?
            .statuses
            .get(&id)
            .copied()
            .ok_or_else(|| NovaError::NotFound(format!("transaction {id}")))
    }

    fn lock(&self) -> Result<MutexGuard<'_, Inner>> {
        self.inner
            .lock()
            .map_err(|_| NovaError::Internal("transaction mutex poisoned".to_owned()))
    }
}

fn ensure_active(inner: &Inner, id: TransactionId) -> Result<()> {
    match inner.statuses.get(&id) {
        Some(TransactionStatus::Active) => Ok(()),
        Some(status) => Err(NovaError::InvalidState(format!(
            "transaction {id} is {status:?}"
        ))),
        None => Err(NovaError::NotFound(format!("transaction {id}"))),
    }
}

fn conflict<T>(collection: &str) -> Result<T> {
    Err(NovaError::InvalidState(format!(
        "lock conflict on collection {collection:?}"
    )))
}

fn release_locks(locks: &mut BTreeMap<String, LockState>, id: TransactionId) {
    locks.retain(|_, lock| {
        lock.readers.remove(&id);
        if lock.writer == Some(id) {
            lock.writer = None;
        }
        !lock.readers.is_empty() || lock.writer.is_some()
    });
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use super::*;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "novadb-transaction-{label}-{}-{}.wal",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn shared_locks_coexist_and_exclusive_locks_conflict() {
        let path = path("locks");
        let manager = TransactionManager::open(&path).unwrap();
        let first = manager.begin().unwrap();
        let second = manager.begin().unwrap();
        manager.acquire_shared(first, "students").unwrap();
        manager.acquire_shared(second, "students").unwrap();
        assert!(matches!(
            manager.acquire_exclusive(first, "students"),
            Err(NovaError::InvalidState(_))
        ));
        manager.rollback(second).unwrap();
        manager.acquire_exclusive(first, "students").unwrap();
        manager.commit(first).unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn terminal_states_are_durable_and_cannot_transition_twice() {
        let path = path("states");
        let manager = TransactionManager::open(&path).unwrap();
        let committed = manager.begin().unwrap();
        manager.commit(committed).unwrap();
        assert_eq!(
            manager.status(committed).unwrap(),
            TransactionStatus::Committed
        );
        assert!(manager.rollback(committed).is_err());
        let rolled_back = manager.begin().unwrap();
        manager.rollback(rolled_back).unwrap();
        assert_eq!(
            manager.status(rolled_back).unwrap(),
            TransactionStatus::RolledBack
        );
        drop(manager);
        let wal = WriteAheadLog::open(&path).unwrap();
        assert_eq!(wal.records().len(), 4);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn manager_is_thread_safe_and_releases_locks_on_commit() {
        let path = path("threads");
        let manager = Arc::new(TransactionManager::open(&path).unwrap());
        let writer = manager.begin().unwrap();
        manager.acquire_exclusive(writer, "students").unwrap();
        let contender = manager.begin().unwrap();
        let other = Arc::clone(&manager);
        let result = std::thread::spawn(move || other.acquire_shared(contender, "students"))
            .join()
            .unwrap();
        assert!(matches!(result, Err(NovaError::InvalidState(_))));
        manager.commit(writer).unwrap();
        manager.acquire_shared(contender, "students").unwrap();
        manager.rollback(contender).unwrap();
        drop(manager);
        fs::remove_file(path).unwrap();
    }
}

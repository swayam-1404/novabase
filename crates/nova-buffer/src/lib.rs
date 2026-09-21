//! Bounded, write-back page buffer for NovaDB.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::Path;

use nova_core::error::{NovaError, Result};
use nova_storage::{Page, PageId, PageManager};

#[derive(Debug)]
struct Frame {
    page: Page,
    dirty: bool,
    pins: usize,
    last_used: u64,
}

/// Observable buffer-pool counters.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BufferStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub writes: u64,
}

/// Capacity-bounded LRU page cache with write-back dirty pages.
#[derive(Debug)]
pub struct BufferPool {
    pages: PageManager,
    frames: BTreeMap<PageId, Frame>,
    capacity: usize,
    clock: u64,
    stats: BufferStats,
}

impl BufferPool {
    /// Opens a page file with a fixed positive frame capacity.
    ///
    /// # Errors
    ///
    /// Returns a typed argument or page-file error.
    pub fn open(path: impl AsRef<Path>, capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(NovaError::InvalidArgument(
                "buffer pool capacity must be positive".to_owned(),
            ));
        }
        Ok(Self {
            pages: PageManager::open(path)?,
            frames: BTreeMap::new(),
            capacity,
            clock: 0,
            stats: BufferStats::default(),
        })
    }

    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    #[must_use]
    pub const fn stats(&self) -> BufferStats {
        self.stats
    }

    /// Allocates a disk page and loads it into the pool.
    ///
    /// # Errors
    ///
    /// Returns a typed allocation, read, write-back, or capacity error.
    pub fn allocate(&mut self) -> Result<PageId> {
        if self.frames.len() == self.capacity {
            self.evict_one()?;
        }
        let id = self.pages.allocate()?;
        self.load(id)?;
        Ok(id)
    }

    /// Fetches a cached page, loading and possibly evicting first.
    ///
    /// # Errors
    ///
    /// Returns a typed read, write-back, or all-frames-pinned error.
    pub fn get(&mut self, id: PageId) -> Result<&Page> {
        self.load(id)?;
        self.frames
            .get(&id)
            .map(|frame| &frame.page)
            .ok_or_else(|| NovaError::Internal("loaded buffer frame disappeared".to_owned()))
    }

    /// Fetches a mutable page and marks its frame dirty.
    ///
    /// # Errors
    ///
    /// Returns a typed read, write-back, or all-frames-pinned error.
    pub fn get_mut(&mut self, id: PageId) -> Result<&mut Page> {
        self.load(id)?;
        let frame = self
            .frames
            .get_mut(&id)
            .ok_or_else(|| NovaError::Internal("loaded buffer frame disappeared".to_owned()))?;
        frame.dirty = true;
        Ok(&mut frame.page)
    }

    /// Pins a page so it cannot be selected for eviction.
    ///
    /// # Errors
    ///
    /// Returns a typed load error or pin-count overflow.
    pub fn pin(&mut self, id: PageId) -> Result<()> {
        self.load(id)?;
        let frame = self
            .frames
            .get_mut(&id)
            .ok_or_else(|| NovaError::Internal("loaded buffer frame disappeared".to_owned()))?;
        frame.pins = frame
            .pins
            .checked_add(1)
            .ok_or_else(|| NovaError::InvalidState("page pin count overflow".to_owned()))?;
        Ok(())
    }

    /// Releases one pin.
    ///
    /// # Errors
    ///
    /// Returns [`NovaError::InvalidState`] if the page is absent or unpinned.
    pub fn unpin(&mut self, id: PageId) -> Result<()> {
        let frame = self
            .frames
            .get_mut(&id)
            .ok_or_else(|| NovaError::InvalidState("page is not buffered".to_owned()))?;
        frame.pins = frame
            .pins
            .checked_sub(1)
            .ok_or_else(|| NovaError::InvalidState("page is not pinned".to_owned()))?;
        Ok(())
    }

    /// Writes every dirty frame and synchronizes the page file.
    ///
    /// # Errors
    ///
    /// Returns a typed page encoding or I/O error.
    pub fn flush_all(&mut self) -> Result<()> {
        for frame in self.frames.values_mut() {
            if frame.dirty {
                self.pages.write(&frame.page)?;
                frame.dirty = false;
                self.stats.writes += 1;
            }
        }
        self.pages.sync()
    }

    /// Flushes and consumes the pool.
    ///
    /// # Errors
    ///
    /// Returns a typed page encoding or I/O error.
    pub fn shutdown(mut self) -> Result<()> {
        self.flush_all()
    }

    fn load(&mut self, id: PageId) -> Result<()> {
        self.clock = self.clock.wrapping_add(1);
        if let Some(frame) = self.frames.get_mut(&id) {
            self.stats.hits += 1;
            frame.last_used = self.clock;
            return Ok(());
        }
        self.stats.misses += 1;
        if self.frames.len() == self.capacity {
            self.evict_one()?;
        }
        let page = self.pages.read(id)?;
        self.frames.insert(
            id,
            Frame {
                page,
                dirty: false,
                pins: 0,
                last_used: self.clock,
            },
        );
        Ok(())
    }

    fn evict_one(&mut self) -> Result<()> {
        let victim = self
            .frames
            .iter()
            .filter(|(_, frame)| frame.pins == 0)
            .min_by_key(|(id, frame)| (frame.last_used, **id))
            .map(|(id, _)| *id)
            .ok_or_else(|| NovaError::InvalidState("all buffer frames are pinned".to_owned()))?;
        let frame = self.frames.remove(&victim).expect("selected frame");
        if frame.dirty {
            self.pages.write(&frame.page)?;
            self.stats.writes += 1;
        }
        self.stats.evictions += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "novadb-buffer-{label}-{}-{}.pages",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn lru_eviction_writes_dirty_pages_and_bounds_memory() {
        let path = path("lru");
        let mut pool = BufferPool::open(&path, 2).unwrap();
        let a = pool.allocate().unwrap();
        let b = pool.allocate().unwrap();
        pool.get_mut(a).unwrap().insert(b"persisted").unwrap();
        pool.get(b).unwrap();
        let c = pool.allocate().unwrap();
        assert_eq!(pool.len(), 2);
        assert_eq!(pool.stats().evictions, 1);
        assert_eq!(pool.stats().writes, 1);
        pool.get(c).unwrap();
        pool.shutdown().unwrap();
        let mut manager = PageManager::open(&path).unwrap();
        assert_eq!(
            manager.read(a).unwrap().get(nova_storage::SlotId::new(0)),
            Some(b"persisted".as_slice())
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn pinned_frames_block_eviction_until_released() {
        let path = path("pins");
        let mut pool = BufferPool::open(&path, 1).unwrap();
        let a = pool.allocate().unwrap();
        pool.pin(a).unwrap();
        assert!(matches!(pool.allocate(), Err(NovaError::InvalidState(_))));
        pool.unpin(a).unwrap();
        pool.allocate().unwrap();
        pool.shutdown().unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn hits_misses_and_invalid_operations_are_explicit() {
        let path = path("stats");
        assert!(matches!(
            BufferPool::open(&path, 0),
            Err(NovaError::InvalidArgument(_))
        ));
        let mut pool = BufferPool::open(&path, 1).unwrap();
        let page = pool.allocate().unwrap();
        pool.get(page).unwrap();
        assert_eq!(pool.stats().hits, 1);
        assert_eq!(pool.stats().misses, 1);
        assert!(pool.unpin(page).is_err());
        pool.shutdown().unwrap();
        fs::remove_file(path).unwrap();
    }
}

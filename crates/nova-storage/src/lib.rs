//! Page-based persistence primitives for NovaDB.
//!
//! Phase 3 provides checksummed fixed-size slotted pages and direct page-file
//! I/O. The higher-level document storage engine is layered on these primitives
//! in Phase 4.

#![forbid(unsafe_code)]

mod checksum;
mod engine;
mod manager;
mod page;

pub use engine::StorageEngine;
pub use manager::PageManager;
pub use page::{
    Page, PageId, SlotId, PAGE_FORMAT_VERSION, PAGE_HEADER_SIZE, PAGE_MAGIC, PAGE_SIZE,
};

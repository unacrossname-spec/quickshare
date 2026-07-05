// Breakpoint resume: track received chunks per transfer in memory.
// On resume, sender skips already-received chunks.
// TODO: persist to SQLite in Phase 1 step 5.

use std::collections::BTreeSet;

/// Tracks which chunks have been received for a transfer.
#[derive(Debug, Default)]
pub struct ResumeTracker {
    received: BTreeSet<u64>,
}

impl ResumeTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark_received(&mut self, index: u64) {
        self.received.insert(index);
    }

    pub fn has(&self, index: u64) -> bool {
        self.received.contains(&index)
    }

    pub fn received_chunks(&self) -> Vec<u64> {
        self.received.iter().copied().collect()
    }
}

// shared/src/crdt.rs
// Thin wrapper around Loro that the relay and daemon both use
// to maintain the shared session document.
//
// Loro gives us:
//   - Movable lists  → track and clip reordering
//   - Trees          → device chains
//   - Per-user undo  → Cmd+Z only undoes your own edits
//   - Version vectors → efficient reconnect / catch-up

use anyhow::Result;
use loro::LoroDoc;

/// The authoritative in-memory document for one session.
/// Both the relay (full copy) and each daemon (local mirror) hold one of these.
pub struct SessionDoc {
    pub doc: LoroDoc,
}

impl SessionDoc {
    /// Create a fresh empty session document.
    pub fn new() -> Self {
        Self {
            doc: LoroDoc::new(),
        }
    }

    /// Apply a Loro update blob received from a remote peer.
    /// Loro merges it into the local document automatically — no conflicts to resolve manually.
    pub fn apply_update(&mut self, update: &[u8]) -> Result<()> {
        self.doc.import(update)?;
        Ok(())
    }

    /// Export everything that has changed since `version` as a compact update blob.
    /// Send this blob to a peer that is catching up after a reconnect.
    pub fn export_since(&self, version: &loro::VersionVector) -> Result<Vec<u8>> {
        Ok(self.doc.export(loro::ExportMode::updates_owned(version.clone()))?)
    }

    /// Export the full document as a snapshot.
    /// Used when a brand-new peer joins and needs the entire session state.
    pub fn export_snapshot(&self) -> Result<Vec<u8>> {
        Ok(self.doc.export(loro::ExportMode::Snapshot)?)
    }

    /// Return the current version vector — used to tell a peer "I have everything up to here".
    pub fn version(&self) -> loro::VersionVector {
        self.doc.oplog_vv()
    }
}

impl Default for SessionDoc {
    fn default() -> Self {
        Self::new()
    }
}

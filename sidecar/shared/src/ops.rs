// shared/src/ops.rs
// Every structural edit to the Live session is expressed as one of these operations.
// They travel inside Envelope::body on the Op channel.
// UUIDs are global — the daemon maps them to Live's own internal IDs locally.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// All possible edits to the shared session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Op {
    // ── MIDI note operations ─────────────────────────────────────────────────
    /// Add a new MIDI note to a clip.
    NoteAdd {
        clip_id: Uuid,
        note_id: Uuid,  // new stable ID for this note
        pitch: u8,      // 0–127, MIDI note number
        start_beat: f64,
        duration_beats: f64,
        velocity: u8,   // 0–127
    },

    /// Change one field of an existing note (pitch, position, length, or velocity).
    NoteSet {
        note_id: Uuid,
        field: NoteField,
        value: f64,
    },

    /// Delete a note.
    NoteDel {
        note_id: Uuid,
    },

    // ── Clip operations ───────────────────────────────────────────────────────
    /// Create a new clip in a track slot or at an arrangement position.
    ClipCreate {
        clip_id: Uuid,
        track_id: Uuid,
        slot_index: u32,
        kind: ClipKind,
    },

    /// Delete a clip.
    ClipDel {
        clip_id: Uuid,
    },

    /// Rename a clip.
    ClipRename {
        clip_id: Uuid,
        name: String,
    },

    // ── Track operations ──────────────────────────────────────────────────────
    /// Move a track to a new position in the session.
    TrackMove {
        track_id: Uuid,
        after_track_id: Option<Uuid>, // None = move to top
    },

    // ── Device / parameter operations ─────────────────────────────────────────
    /// Commit the final value of a device parameter after a gesture ends.
    /// (In-progress drag values travel as Gesture packets, not ops.)
    ParamCommit {
        device_id: Uuid,
        param_id: Uuid,
        value: f64,
    },

    // ── Session-level operations ───────────────────────────────────────────────
    /// Change the session tempo.
    TempoSet {
        bpm: f64,
    },
}

/// Which field of a MIDI note is being changed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum NoteField {
    Pitch,
    StartBeat,
    DurationBeats,
    Velocity,
}

/// Whether a clip holds MIDI or audio.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ClipKind {
    Midi,
    Audio,
}

// shared/src/envelope.rs
// Every packet travelling over the wire is wrapped in this envelope.
// postcard serializes it to tight binary — typically 30-60 bytes for a note op.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The kind of message in this envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum MessageKind {
    /// Reliable, ordered — join/leave, permissions, session snapshots
    Control = 0,
    /// Reliable, ordered — CRDT ops (notes, clips, tracks, params)
    Op = 1,
    /// Datagram, latest-wins — selected track, open clip, playhead position
    Presence = 2,
    /// Datagram, latest-wins — in-progress knob/fader drags
    Gesture = 3,
    /// Reliable — play, stop, scene launch, NTP clock sync
    Transport = 4,
    /// Reliable, bulk, lowest priority — sample file transfers
    Blob = 5,
}

/// The outer wrapper for every packet, regardless of channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    /// Protocol version — bump when the format changes incompatibly.
    pub version: u8,

    /// Which kind of payload is inside.
    pub kind: MessageKind,

    /// Session this packet belongs to (128-bit UUID).
    pub session_id: Uuid,

    /// The peer who authored this packet.
    pub peer_id: Uuid,

    /// Monotonically increasing per-peer sequence number (reliable channels only).
    /// Receivers use this to detect gaps and request retransmission.
    pub seq: u64,

    /// Hybrid Logical Clock timestamp — orders concurrent edits correctly
    /// even when wall clocks drift between machines.
    pub hlc: u64,

    /// Binary-encoded payload. Decoded according to `kind`.
    pub body: Vec<u8>,
}

impl Envelope {
    pub fn new(
        kind: MessageKind,
        session_id: Uuid,
        peer_id: Uuid,
        seq: u64,
        hlc: u64,
        body: Vec<u8>,
    ) -> Self {
        Self {
            version: 1,
            kind,
            session_id,
            peer_id,
            seq,
            hlc,
            body,
        }
    }

    /// Serialize to bytes using postcard (no-std compatible, very compact).
    pub fn to_bytes(&self) -> anyhow::Result<Vec<u8>> {
        Ok(postcard::to_allocvec(self)?)
    }

    /// Deserialize from raw bytes received off the wire.
    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        Ok(postcard::from_bytes(bytes)?)
    }
}

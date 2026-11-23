// daemon/src/main.rs
// The per-user sidecar daemon — runs on each collaborator's machine alongside Ableton.
//
// Responsibilities:
//   1. Connect to the relay (or directly to peers) over iroh QUIC.
//   2. Maintain a local CRDT mirror of the shared session.
//   3. Receive ops from the relay and forward to the TypeScript Extension via local socket.
//   4. Receive changes from the Python Remote Script and send as ops to the relay.
//   5. Handle reconnects transparently using version vectors.

use anyhow::Result;
use iroh::{endpoint::presets, EndpointAddr, PublicKey};
use shared::crdt::SessionDoc;
use shared::envelope::{Envelope, MessageKind};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info};
use uuid::Uuid;

const ALPN: &[u8] = b"ableton-multiplayer/1";

struct DaemonState {
    doc: SessionDoc,
    peer_id: Uuid,
    session_id: Uuid,
    seq: u64,
}

impl DaemonState {
    fn new(session_id: Uuid) -> Self {
        Self {
            doc: SessionDoc::new(),
            peer_id: Uuid::new_v4(),
            session_id,
            seq: 0,
        }
    }
    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("daemon=debug,info")
        .init();

    let session_id = Uuid::new_v4();
    info!("Daemon starting. Session: {session_id}");

    let state = Arc::new(RwLock::new(DaemonState::new(session_id)));

    // Channel: outbound ops flowing daemon → relay.
    let (outbound_tx, outbound_rx) = mpsc::channel::<Vec<u8>>(256);
    // Channel: inbound ops relay → Extension (to write into Ableton).
    let (inbound_tx, _inbound_rx) = mpsc::channel::<Vec<u8>>(256);

    // ── Task 1: iroh connection to relay ──────────────────────────────────────
    {
        let state = Arc::clone(&state);
        let inbound_tx = inbound_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = relay_connection(state, inbound_tx, outbound_rx).await {
                error!("Relay connection error: {e}");
            }
        });
    }

    // ── Task 2: Local TCP socket for the TypeScript Extension ─────────────────
    // Extension connects here to push confirmed edits back to the daemon.
    {
        let extension_listener = TcpListener::bind("127.0.0.1:7878").await?;
        info!("Extension socket listening on 127.0.0.1:7878");
        let state = Arc::clone(&state);
        let outbound_tx = outbound_tx.clone();
        tokio::spawn(async move {
            loop {
                match extension_listener.accept().await {
                    Ok((socket, addr)) => {
                        info!("Extension connected from {addr}");
                        let state = Arc::clone(&state);
                        let tx = outbound_tx.clone();
                        tokio::spawn(handle_extension(socket, state, tx));
                    }
                    Err(e) => error!("Extension accept error: {e}"),
                }
            }
        });
    }

    // ── Task 3: Local TCP socket for the Python Remote Script ─────────────────
    // Remote Script connects here to push Live change events.
    let script_listener = TcpListener::bind("127.0.0.1:7879").await?;
    info!("Remote Script socket listening on 127.0.0.1:7879");

    loop {
        match script_listener.accept().await {
            Ok((socket, addr)) => {
                info!("Remote Script connected from {addr}");
                let state = Arc::clone(&state);
                let tx = outbound_tx.clone();
                tokio::spawn(handle_remote_script(socket, state, tx));
            }
            Err(e) => error!("Remote Script accept error: {e}"),
        }
    }
}

/// Manage the iroh connection to the relay for the entire session lifetime.
async fn relay_connection(
    state: Arc<RwLock<DaemonState>>,
    inbound_tx: mpsc::Sender<Vec<u8>>,
    mut outbound_rx: mpsc::Receiver<Vec<u8>>,
) -> Result<()> {
    // Build our endpoint. presets::N0 handles discovery, NAT traversal, relay fallback.
    let endpoint = iroh::Endpoint::builder(presets::N0)
        .bind()
        .await?;

    info!("Daemon node ID: {}", endpoint.id());

    // The relay's PublicKey (EndpointId) comes from the env var.
    // In production this comes from the room code entered in the panel UI.
    let relay_key: PublicKey = std::env::var("RELAY_NODE_ID")
        .expect("Set RELAY_NODE_ID to the relay's node ID printed at relay startup")
        .parse()?;

    // EndpointAddr::from(PublicKey) lets iroh resolve the relay's current address via DNS.
    let relay_addr = EndpointAddr::from(relay_key);

    info!("Connecting to relay {}...", relay_key);
    let conn = endpoint.connect(relay_addr, ALPN).await?;
    info!("Connected to relay.");

    let (mut send, mut recv) = conn.open_bi().await?;

    // Send the join packet (Control kind, carries our session ID).
    let (session_id, peer_id) = {
        let s = state.read().await;
        (s.session_id, s.peer_id)
    };
    let join = Envelope::new(MessageKind::Control, session_id, peer_id, 0, 0, vec![]);
    send.write_all(&join.to_bytes()?).await?;
    info!("Join sent for session {session_id}");

    // Spawn outbound sender — drains the outbound channel and writes to the relay stream.
    tokio::spawn(async move {
        while let Some(packet) = outbound_rx.recv().await {
            if send.write_all(&packet).await.is_err() {
                break;
            }
        }
    });

    // Read inbound packets from relay, merge into CRDT, forward to Extension.
    let mut buf = vec![0u8; 65536];
    loop {
        let n = match recv.read(&mut buf).await {
            Ok(Some(n)) => n,
            _ => break,
        };

        let envelope = match Envelope::from_bytes(&buf[..n]) {
            Ok(e) => e,
            Err(e) => { error!("Bad envelope from relay: {e}"); continue; }
        };

        match envelope.kind {
            MessageKind::Control => {
                // First Control from relay is the session snapshot.
                info!("Received session snapshot ({} bytes)", envelope.body.len());
                let mut s = state.write().await;
                s.doc.apply_update(&envelope.body)?;
            }
            MessageKind::Op => {
                // Merge remote op into local CRDT mirror, then forward to Extension.
                {
                    let mut s = state.write().await;
                    s.doc.apply_update(&envelope.body)?;
                }
                let _ = inbound_tx.send(buf[..n].to_vec()).await;
            }
            _ => {}
        }
    }

    info!("Relay connection closed.");
    Ok(())
}

/// Bridge between TypeScript Extension and the relay.
async fn handle_extension(
    mut socket: tokio::net::TcpStream,
    state: Arc<RwLock<DaemonState>>,
    outbound_tx: mpsc::Sender<Vec<u8>>,
) {
    let mut buf = vec![0u8; 65536];
    loop {
        let n = match socket.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let (session_id, peer_id, seq) = {
            let mut s = state.write().await;
            let seq = s.next_seq();
            (s.session_id, s.peer_id, seq)
        };
        let env = Envelope::new(MessageKind::Op, session_id, peer_id, seq, 0, buf[..n].to_vec());
        if let Ok(bytes) = env.to_bytes() {
            let _ = outbound_tx.send(bytes).await;
        }
    }
}

/// Bridge between Python Remote Script and the relay.
async fn handle_remote_script(
    mut socket: tokio::net::TcpStream,
    state: Arc<RwLock<DaemonState>>,
    outbound_tx: mpsc::Sender<Vec<u8>>,
) {
    let mut buf = vec![0u8; 65536];
    loop {
        let n = match socket.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let (session_id, peer_id, seq) = {
            let mut s = state.write().await;
            let seq = s.next_seq();
            (s.session_id, s.peer_id, seq)
        };
        // Stage 2: diff against CRDT mirror here to produce minimal ops.
        // For now: forward raw bytes as Op body.
        let env = Envelope::new(MessageKind::Op, session_id, peer_id, seq, 0, buf[..n].to_vec());
        if let Ok(bytes) = env.to_bytes() {
            let _ = outbound_tx.send(bytes).await;
        }
    }
}

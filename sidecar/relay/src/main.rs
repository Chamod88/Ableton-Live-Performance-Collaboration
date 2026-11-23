// relay/src/main.rs
// The hollow relay server.
//
// Responsibilities:
//   1. Accept connections from daemons over iroh (QUIC + NAT traversal).
//   2. Manage sessions — create, join, track membership.
//   3. Fan out Op and Presence packets to all peers in a session.
//   4. Hold the full CRDT so a reconnecting peer can catch up.
//   5. Store nothing to disk — all state is in RAM.
//
// What it deliberately does NOT do:
//   - Store project files or samples.
//   - Read the contents of Op packets (it fans them out opaque).
//   - Authenticate users beyond a session code (v1).

use anyhow::Result;
use iroh::endpoint::presets;
use shared::crdt::SessionDoc;
use shared::envelope::{Envelope, MessageKind};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{broadcast, RwLock};
use tracing::{error, info};
use uuid::Uuid;

/// ALPN protocol identifier — iroh uses this to route connections to the right handler.
const ALPN: &[u8] = b"ableton-multiplayer/1";

/// One active collaboration session.
struct Session {
    /// The authoritative CRDT document — used for catch-up on reconnect.
    doc: SessionDoc,
    /// Broadcast channel — every packet sent here goes to all peers in the session.
    tx: broadcast::Sender<Vec<u8>>,
}

impl Session {
    fn new() -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self { doc: SessionDoc::new(), tx }
    }
}

type Sessions = Arc<RwLock<HashMap<Uuid, Session>>>;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("relay=debug,info")
        .init();

    info!("Starting Ableton Multiplayer relay...");

    // iroh 1.x: builder takes a Preset. presets::N0 sets up DNS discovery + relay fallback.
    // We also register the ALPN so only our protocol's connections are accepted.
    let endpoint = iroh::Endpoint::builder(presets::N0)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await?;

    // iroh 1.x: endpoint.id() returns EndpointId (= PublicKey).
    let node_id = endpoint.id();
    info!("Relay node ID: {node_id}");

    println!("\n╔══════════════════════════════════════════════════════════════════╗");
    println!("║  RELAY NODE ID (share with collaborators to join your sessions)  ║");
    println!("║  {node_id}  ║");
    println!("╚══════════════════════════════════════════════════════════════════╝\n");

    let sessions: Sessions = Arc::new(RwLock::new(HashMap::new()));

    // iroh 1.x: endpoint.accept() returns Accept<'_> which is a Future (not a Stream).
    // Call it fresh each iteration — it resolves to Option<Incoming>.
    loop {
        match endpoint.accept().await {
            Some(incoming) => {
                let sessions = Arc::clone(&sessions);
                tokio::spawn(async move {
                    // incoming.await completes the QUIC handshake → gives us a Connection.
                    match incoming.await {
                        Ok(conn) => {
                            if let Err(e) = handle_connection(conn, sessions).await {
                                error!("Connection error: {e}");
                            }
                        }
                        Err(e) => error!("Handshake failed: {e}"),
                    }
                });
            }
            None => {
                info!("Relay endpoint closed.");
                break;
            }
        }
    }

    Ok(())
}

async fn handle_connection(
    conn: iroh::endpoint::Connection,
    sessions: Sessions,
) -> Result<()> {
    let peer_id = Uuid::new_v4();
    info!("Peer connected: {peer_id}");

    let (mut send, mut recv) = conn.accept_bi().await?;

    // First message must be a Control join packet carrying the session ID.
    let mut buf = vec![0u8; 65536];
    let n = recv.read(&mut buf).await?.unwrap_or(0);
    if n == 0 {
        return Ok(());
    }

    let envelope = Envelope::from_bytes(&buf[..n])?;
    let session_id = envelope.session_id;
    info!("Peer {peer_id} joining session {session_id}");

    {
        let mut sessions = sessions.write().await;
        sessions.entry(session_id).or_insert_with(Session::new);
    }

    let mut rx = {
        let sessions = sessions.read().await;
        sessions[&session_id].tx.subscribe()
    };

    // Send current session snapshot to the joining peer.
    {
        let sessions = sessions.read().await;
        let snapshot = sessions[&session_id].doc.export_snapshot()?;
        let snap_env = Envelope::new(
            MessageKind::Control,
            session_id,
            Uuid::nil(),
            0, 0,
            snapshot,
        );
        send.write_all(&snap_env.to_bytes()?).await?;
        info!("Sent snapshot ({} bytes) to peer {peer_id}", snap_env.body.len());
    }

    // Forward broadcast packets back to this peer.
    let forward_task = tokio::spawn(async move {
        while let Ok(packet) = rx.recv().await {
            if send.write_all(&packet).await.is_err() {
                break;
            }
        }
    });

    // Read and fan out incoming packets.
    loop {
        let n = match recv.read(&mut buf).await {
            Ok(Some(n)) => n,
            _ => break,
        };

        let envelope = match Envelope::from_bytes(&buf[..n]) {
            Ok(e) => e,
            Err(e) => { error!("Bad envelope: {e}"); continue; }
        };

        // Merge Op packets into the authoritative CRDT for reconnect replay.
        if envelope.kind == MessageKind::Op {
            let mut sessions = sessions.write().await;
            if let Some(session) = sessions.get_mut(&session_id) {
                if let Err(e) = session.doc.apply_update(&envelope.body) {
                    error!("CRDT merge failed: {e}");
                }
            }
        }

        let packet = buf[..n].to_vec();
        let sessions = sessions.read().await;
        if let Some(session) = sessions.get(&session_id) {
            let _ = session.tx.send(packet);
        }
    }

    info!("Peer {peer_id} disconnected.");
    forward_task.abort();
    Ok(())
}

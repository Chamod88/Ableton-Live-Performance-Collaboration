# Ableton Multiplayer

P2P collaborative DAW session syncing for Ableton Live using a Rust sidecar, Loro CRDT, and Ableton Remote Scripts.

## Architecture
- `sidecar/`: Rust workspace (`shared`, `daemon`, `relay`).
  - `shared`: Envelope protocols, edit ops, Loro CRDT state wrappers.
  - `daemon`: Local machine agent bridging Ableton Remote Script ↔ Relay ↔ Extension.
  - `relay`: P2P connection coordinator and fallback encrypted packet fanout.
- `remote-script/`: Ableton Python Remote Script interacting with Live Object Model (LOM).
- `extension/` & `panel/`: UI frontend for room codes, peers, and session controls.

## Development & Build Commands
- Check Rust: `cargo check --manifest-path sidecar/Cargo.toml`
- Build daemon: `cargo build --manifest-path sidecar/Cargo.toml -p daemon`
- Test protocol: `cargo test --manifest-path sidecar/Cargo.toml`

## Guidelines for AI Agents
- Preserve P2P encryption: the relay must never inspect unencrypted session or project data.
- Treat Loro CRDT operations as append-only deltas; avoid mutating historical op logs directly.
- Maintain minimal edits and keep Rust async runtime calls non-blocking.

Forked Goose cleanly

Made ag1goose/ as a sibling repo, sharing Git objects and Cargo build cache.

Result: no duplicate source blobs or gigantic target/ dirs.

Turned it into a Cargo workspace

Root Cargo.toml lists all member crates (crates/*) and sets resolver = "2".

This lets you add your own crates (like bus, ag1_meta) without touching upstream ones.

Created two new crates:

bus/: a Redis Stream adapter that knows how to send/receive your canonical JSON Envelope (from envelope.py).

We serialize the entire envelope into a single Redis field "env" to avoid schema drift.

Functions:

send(stream, &Envelope) -> id

recv_block(stream, last_id, block_ms) -> Option<Envelope>

Includes a unit test that round-trips through Redis.

ag1_meta/: a helper (extension) layer that will expose a delegate() function for Goose to call.

It builds an Envelope, sends it via bus::send, then blocks waiting for a reply on another stream.

Uses anyhow for simple error handling.

Compiled & tested successfully

cargo test -p bus passed.

cargo build -p ag1_meta built both debug and release.

Aligned Rust with your Python bus schema

Rust Envelope mirrors your Python envelope.py fields (role, content, meta, etc.).

We removed msg and kept content["text"] (as your agents use).

Optional/unknown fields are handled via #[serde(default)].
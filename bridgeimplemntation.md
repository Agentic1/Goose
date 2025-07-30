Here’s a quick, inline breakdown of what’s changed in your latest bridge.rs and session.rs, and what’s still missing or behaving unexpectedly:

bridge.rs 
Polling loop tweaks

last_id is now initialized to "$" (so you only see new messages).

Block timeout hard‑coded to 2000ms in recv_block instead of using your turn_timeout_ms setting.

Session management

On each envelope, you extract or generate a session_id (sid) and store it in reply_to_session.

You call get_or_start_session(&sid) to ensure there’s a running Goose process.

Debug vs. log

You’re still mostly using println! for debug—mixed with tracing macros (info!, error!, etc.). Consider standardizing on one.

Missing response dispatch

After handle_envelope sends the user input into the Goose CLI, there’s no code that reads from the session’s JSONL log or the CLI’s stdout and pushes the assistant’s reply back onto Redis. That’s why you never see responses in your relay.

Duped reply‑to logic

You compute reply_to twice in handle_envelope. You can collapse that into one block at start.

session.rs 
Session log path

You now reliably build and create ~/.local/share/goose/sessions/<sid>.jsonl.

Process spawning

The Goose CLI is launched with --name, built‑in flags, and any MCP extensions.

Stdout/stderr are piped, and stderr lines about the goose_agent extension are filtered.

State tracking stubbed

You’ve defined a SessionState (with last_processed_pos, etc.) but never actually update or use it.

No JSONL tailer

There’s still no implementation to tail the session’s JSONL file (e.g. opening it, seeking to last_processed_pos, reading new lines, parsing them, and returning the assistant’s message).

Why you’re “in a loop”
Because bridge.rs never pulls the assistant’s reply back out of the session, every incoming user message kicks off a spawn/send cycle, but the bridge never “hears” the reply—so it just sits waiting again and again.

Next steps to get it working:
Implement JSONL reader in GooseSession:

Keep track of your last read byte offset.

After writing to stdin, asynchronously tail the JSONL file for the next {"role":"assistant",…} entry.

Return that text so handle_envelope can package it into an Envelope and push it to Redis.

Unify logging:

Switch your println! calls to debug! or info! from tracing so you can control verbosity with RUST_LOG.

Clean up duplicate logic:

Dedupe the reply_to extraction.

Remove the unused SessionState (or actually use it).

Once you wire up the JSONL‐tail reader and dispatch the assistant’s messages, your bridge will start echoing Goose’s replies back through your relay.
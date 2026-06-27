# sily

**Git-like commit / branch / revert for AI sessions.**

You're deep in a Claude Code session and it's in a good state. You `sily commit`
it. You keep working — and it goes sideways. You `sily revert`, and you're back
at the good point, with the bad branch preserved. No prompts, no API calls — sily
works directly on the session files on disk.

## How it works

A Claude Code session is a single `.jsonl` append-log at
`~/.claude/projects/<encoded-cwd>/<session-uuid>.jsonl`. sily reads that log,
slices it at any message, and writes a new valid, resumable session — so a
"branch" or "revert" is just a new session you `claude --resume`.

A **commit** is a tiny named pointer (`session_id` + `message_uuid`) stored in
`~/.sily/`, never a copy of the conversation — so commits cost almost nothing
regardless of session size.

## Architecture

Ports-and-adapters (hexagonal), so other providers can slot in later:

| Crate | Role |
|-------|------|
| `sily-core` | Provider-agnostic model (`Session`, `Message`, `Commit`) and the pure branch/revert/diff operations. No I/O. |
| `sily-adapter-claude` | Implements the `SessionStore` port against Claude Code's `.jsonl` files. The only crate that knows Claude's format. |
| `sily-cli` | The `sily` binary; wires commands onto core + adapter and the `~/.sily` commit store. |

A new provider is a new `sily-adapter-*` crate implementing `SessionStore` —
nothing in `sily-core` changes.

## Install

One-line install (Linux x86_64, macOS arm64/x86_64):

```bash
curl -fsSL https://raw.githubusercontent.com/AmitsinghTanwar007/Sily/main/install.sh | sh
```

Installs the latest released binary to `~/.local/bin` (override with `SILY_BIN_DIR`).

With Rust toolchain, from source:

```bash
cargo install --git https://github.com/AmitsinghTanwar007/Sily sily-cli
# or locally:
cargo build --release   # binary at target/release/sily
```

## Usage

```bash
sily list                                   # sessions in the current project
sily log <session>                          # history (append-log order)
sily tree <session>                         # branch structure
sily commit <session> [--name x] [-m note] [--at <msg>]
sily commits                                # list saved commits
sily branch <session> [--at <msg>]          # → new session, prints resume cmd
sily revert <commit> [--hard]               # soft = fork (default), hard = reset
sily diff <a> <b>                           # where two sessions diverge
```

Then resume the session sily prints:

```bash
claude --resume <new-session-id>
```

### Environment

- `SILY_CLAUDE_HOME` — override the Claude home (default `~/.claude`).
- `SILY_HOME` — override the sily metadata home (default `~/.sily`).

## Design notes (learned from real sessions)

- **File order is the source of truth.** Real session files are append-logs whose
  `parentUuid` links reference messages never written to the file (compaction
  boundaries, tool/meta records). Claude reconstructs by file order, so sily
  slices by file order too; `parentUuid` is kept but treated as best-effort
  metadata.
- **Malformed lines happen.** Interrupted writes can leave a corrupt line; `load`
  skips it (with a warning) instead of failing the whole session.
- **`sessionId` is rewritten on save** so a branched/reverted file is a valid,
  resumable session.

## Status

Core, Claude adapter, and CLI are working and tested end-to-end against real
sessions. See the issue tracker for polish items (smarter HEAD selection, header
pruning, corrupt-line recovery).

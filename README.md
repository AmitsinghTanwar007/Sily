# sily

**Save and restore your Claude Code sessions — like git, but for AI chats.**

Working in a Claude Code session that's in a good state? Save it with `sily commit`.
Keep going — and if it goes wrong, `sily revert` puts you right back at the good
point. No copy-paste, no losing work.

---

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/AmitsinghTanwar007/Sily/main/install.sh | sh
```

That's it — installs to `/usr/local/bin` (already on your PATH, so `sily` works
right away; may ask for `sudo`).

Prefer not to use root? Install to a user directory instead:

```bash
SILY_BIN_DIR="$HOME/.local/bin" curl -fsSL https://raw.githubusercontent.com/AmitsinghTanwar007/Sily/main/install.sh | sh
```

(That auto-adds the directory to your shell PATH; run `source ~/.bashrc` once, or
open a new terminal.)

---

## Quick start

```bash
# 1. See your sessions
sily list

# 2. Save a good point (a "commit")
sily commit <session-id> -m "working great here"

# 3. ...keep working in Claude. If it goes sideways:

# 4. Go back — this prints a new session id
sily revert <commit-name>

# 5. Resume that session in Claude
claude --resume <new-session-id>
```

You're back at the good point. Your messed-up version is still saved too — nothing
is ever lost.

---

## All commands

| Command | What it does |
|---------|--------------|
| `sily list` | Interactive collapsible tree of all projects (static when piped) |
| `sily list --here` | Just the current folder's sessions (static) |
| `sily log <session>` | Show a session's messages |
| `sily tree <session>` | Show a session's branch structure |
| `sily commit <session> [-m note] [--name x] [--at <msg>]` | Save a point you can return to |
| `sily commits` | List your saved points |
| `sily branch <session> [--at <msg>]` | Make a new session from any point |
| `sily revert <commit> [--hard]` | Go back to a saved point (default: keeps old version) |
| `sily diff <a> <b>` | Show where two sessions differ |
| `sily update` | Update sily to the latest release |

In the interactive `sily list` (in a terminal): `↑`/`↓` move, `→`/`Enter` expand
(everything starts collapsed), `←` collapse, `y` copy the selected session's
`claude --resume` command, `q` quit.

Tips:
- A **commit** is just a tiny bookmark (a pointer), not a copy — save as many as you like.
- `revert` is **safe by default**: it creates a *new* session and leaves everything else
  intact. Use `--hard` only if you want to truly discard the later messages.
- Most commands take an optional `--at <message-id>` to act on an exact point instead
  of the latest one.

---

## How it works (short version)

A Claude Code session is just a file on disk. sily reads that file, slices it at the
point you choose, and writes a new valid session you can resume — all without calling
any API. Your commits live in `~/.sily/`.

Built in Rust as a clean core + pluggable adapters. `sily list` shows sessions from
**Claude Code**, **Codex CLI** (`~/.codex/sessions`), and **OpenCode** (its SQLite
database) together in one tree.

Commit / branch / revert:
- **Claude Code** — full support (writes a new `.jsonl`; `claude --resume`).
- **Codex CLI** — full support (writes a new rollout; `codex resume`). Branch points
  are message numbers (`--at 3`).
- **OpenCode** — branch/revert via OpenCode's own `export`/`import` (no direct DB
  writes); branch points are message ids. Experimental — verify the result.

---

## License

MIT

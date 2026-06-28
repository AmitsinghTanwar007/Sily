# sily

**Save and restore your AI coding sessions — like git, but for AI chats.**

Works across **Claude Code**, **Codex CLI**, and **OpenCode** — one tool to browse,
bookmark, and rewind sessions from any of them.

In a session that's in a good state? Save it with `sily commit`. Keep going — and if
it goes wrong, `sily revert` puts you right back at the good point, with the bad
version still kept. No copy-paste, no losing work.

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
# 1. See your sessions (from every supported tool)
sily list

# 2. Save a good point (a "commit")
sily commit <session-id> -m "working great here"

# 3. ...keep working. If it goes sideways:

# 4. Go back — this prints a new session id AND the exact resume command
sily revert <commit-name>

# 5. Resume that session — sily prints the right command for the tool, e.g.:
claude --resume <id>      # Claude Code
codex resume <id>         # Codex CLI
opencode --session <id>   # OpenCode
```

You're back at the good point. Your messed-up version is still saved too — nothing
is ever lost.

---

## All commands

| Command | What it does |
|---------|--------------|
| `sily list` | Interactive tree of sessions under the current directory (static when piped) |
| `sily list --all` | Every project on the machine |
| `sily log <session>` | Show recent messages (last 8; `--full` for all) |
| `sily log <session> -p` | Show only *your* prompts (skip assistant/tools/noise) |
| `sily tree <session>` | Show recent branch structure (last 8; `--full` for all) |
| `sily graph <session>` | GitHub-style rail: branches/commits split off the timeline at their exact point |
| `sily commit <session> -m "note" [--name x] [--at <msg>]` | Save a point you can return to (message required) |
| `sily commits` | List your saved points |
| `sily branch <session> [--at <msg>]` | Make a new session from any point |
| `sily revert <commit> [--hard]` | Go back to a saved point (default: keeps old version) |
| `sily merge <branch> [--into <session>]` | Combine a branch into its main — or into another branch via `--into` (shared base + both sides' work) |
| `sily diff <a> <b>` | Show where two sessions differ |
| `sily port <session>` | Copy a session's content into a new session in **another** tool (prompts for the target) |
| `sily update` | Update sily to the latest release |

In the interactive `sily list` (in a terminal): browse the tree on the left and the
**selected session's graph** shows on the right — the same **multi-lane rail** as
`sily graph`: newest-first, **noise filtered** (no `/exit`, `/compact`, tool/system
plumbing), each branch in its own **parallel lane** from its fork point, and
just-created branches as a `╰○` stub on the message they forked from. Speakers are
labelled `you` / `ai`.
Keys: `↑`/`↓` move, `→`/`Enter` expand (everything starts collapsed), `←` collapse,
`y` copy the selected session's resume command, `r` reload (pick up changes made
elsewhere), `q` quit.

Lists show **newest first** (`sily commits`, `sily log`).

Tips:
- A **commit** is just a tiny bookmark (a pointer), not a copy — save as many as you like.
- `revert` is **safe by default**: it creates a *new* session and leaves everything else
  intact. Use `--hard` only if you want to truly discard the later messages.
- Most commands take an optional `--at <message-id>` to act on an exact point instead
  of the latest one.

---

## How it works (short version)

Each tool keeps its sessions on disk — Claude Code and Codex as JSONL files, OpenCode
in a SQLite database. sily reads those, slices a session at the point you choose, and
produces a new session you can resume — all without calling any API. Your commits
(tiny pointers) live in `~/.sily/`.

Built in Rust as a clean core + pluggable adapters. Every tool is **one
`impl Provider`** (a trait in `sily-core`), so the CLI is identical across tools and
adding a new one is a single adapter crate.

| Tool | List / browse | Commit / branch / revert | Branch point | Resume |
|------|:---:|:---:|------|--------|
| **Claude Code** | ✅ | ✅ | message id | `claude --resume <id>` |
| **Codex CLI** | ✅ | ✅ | message number (`--at 3`) | `codex resume <id>` |
| **OpenCode** | ✅ | ✅ (experimental, via its own `export`/`import`) | message id | `opencode --session <id>` |
| **Gemini CLI** | ✅ | — | — | `gemini --resume` |
| **Pi** | ✅ (incl. tree) | — | message id | `pi --resume <id>` |

Gemini is listing-only (its `logs.json` records only user prompts). Pi is read-only
for now (full list/log/tree, but branch/port unverified).

**Write operations** (`branch`, `revert`, `merge`, `port`) work for **Claude Code,
Codex CLI, and OpenCode**. Claude is fully verified; Codex and OpenCode writes are
**experimental** — they produce new sessions via each tool's own format/import, so
confirm the resumed session looks right. `merge` works branch→main *and*
branch→branch (it finds the shared base and appends the other side's work).

Where each tool's data lives: Claude `~/.claude`, Codex `~/.codex/sessions`, OpenCode
its SQLite db (`~/.local/share/opencode`), Gemini `~/.gemini/tmp/*/logs.json`, Pi
`~/.pi/agent/sessions`. Override with `SILY_CLAUDE_HOME`, `SILY_CODEX_HOME`,
`SILY_OPENCODE_DB`, `SILY_GEMINI_HOME`, `SILY_PI_DIR`.

### Move a session between tools

`sily port <session>` copies a session's conversation into a **new session in a
different tool** — e.g. continue a Codex session over in OpenCode:

```bash
sily port <codex-session-id>      # prompts: which provider? → opencode
# → ported N messages → opencode session ...   resume with: opencode --session <id>
```

It carries the conversation as readable context (the new session opens knowing what
happened); tool-specific execution state doesn't transfer. (`--to <provider>` skips
the prompt; OpenCode target is experimental — verify the result.)

---

## License

MIT

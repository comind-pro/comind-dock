---
name: cdock
description: Use when running inside comind-dock (the CDOCK_PANE_ID env var is set) and you need another terminal - run something in parallel, watch a long build, test a TUI, or coordinate with other agent panes. Lets you spawn sibling panes, run commands in them, read their screens, and block until output or an agent status appears.
---

# Driving comind-dock from inside a pane

You are inside a pane of the comind-dock terminal multiplexer. The `cdock`
CLI (at `$CDOCK_BIN`; your pane id is `$CDOCK_PANE_ID`) controls the whole
session over a local socket. Every command prints one JSON object; `"ok"`
tells you if it worked (exit code matches).

## Spawn a sibling pane and use it

```bash
# split this pane; the reply carries the new pane id
"$CDOCK_BIN" pane split "$CDOCK_PANE_ID" --direction right
# → {"ok":true,"pane":7}

# or spawn it already running a command
"$CDOCK_BIN" pane split "$CDOCK_PANE_ID" --direction down --command "cargo test"
```

## Run and wait

```bash
"$CDOCK_BIN" pane run 7 "cargo test"                 # types the command + Enter
"$CDOCK_BIN" wait output 7 --match "test result" --timeout 300000
"$CDOCK_BIN" pane read 7 --lines 40                  # → {"ok":true,"text":"..."}
```

`wait output` exits 1 on timeout. `--match` is a plain case-sensitive
substring of the visible screen.

## Orient yourself

```bash
"$CDOCK_BIN" api snapshot     # workspaces → tabs → panes, one JSON tree
"$CDOCK_BIN" pane list        # flat pane list with statuses
"$CDOCK_BIN" agent list       # only recognized agent panes
```

## Coordinate with other agents

```bash
"$CDOCK_BIN" wait agent-status 5 --status idle --timeout 600000
"$CDOCK_BIN" pane send-text 5 "review the diff in src/"   # no Enter
"$CDOCK_BIN" pane send-text 5 $'\r'                        # Enter separately
```

Agent statuses: `working`, `blocked` (needs human input), `done`, `idle`,
`unknown`.

## Rules

- Clean up panes you spawned when done: `"$CDOCK_BIN" pane focus <id>` the
  user can see it, or leave long-running watchers only if the user asked.
- Never `pane run` into a pane whose program you don't know — check
  `pane list` first; typing into another agent's pane injects text into
  their conversation.
- Pane ids are numbers; `%7` and `7` both work as arguments.

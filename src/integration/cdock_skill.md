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
"$CDOCK_BIN" api reference    # every socket API command with an example
```

## Watch instead of polling

```bash
"$CDOCK_BIN" events --only agent-status         # JSON line per transition
"$CDOCK_BIN" pane observe 7                     # raw output stream of a pane
```

## Structure

```bash
"$CDOCK_BIN" tab create                          # new tab, reply has pane id
"$CDOCK_BIN" workspace create --cwd ~/proj/x     # new space in a folder
"$CDOCK_BIN" agent start "claude" --split right  # spawn an agent
"$CDOCK_BIN" agent start --profile reviewer      # spawn by profile
```

## Define your own subagents (workspace-scoped)

You can author agent roles for THIS workspace and spawn subagents with
them. They live in cdock metadata (not the repo), keyed to this folder:

```bash
"$CDOCK_BIN" profile new researcher --ws          # scaffold, prints the dir
# then write <dir>/agent.md — the role/system prompt (and optionally
# profile.toml: command, env, skills = ["..."] from the skill catalog)
"$CDOCK_BIN" profile list                         # ws:researcher + globals
"$CDOCK_BIN" agent start --profile researcher --split right
# bare names prefer this workspace's agents; ws:/global: pick explicitly
"$CDOCK_BIN" agent behavior 7 ws:researcher       # inject a role into a
                                                  # RUNNING agent pane
"$CDOCK_BIN" pane rename 7 "kafka consumers"     # name the session (shown
                                                  # in the sidebar)
```

## Coordinate with other agents

```bash
"$CDOCK_BIN" wait agent-status 5 --status idle --timeout 600000
"$CDOCK_BIN" pane send-text 5 "review the diff in src/"   # no Enter
"$CDOCK_BIN" pane send-text 5 $'\r'                        # Enter separately
"$CDOCK_BIN" pane send-text 5 "line1
line2" --paste                                             # multiline-safe block
```

Agent statuses: `working`, `blocked` (needs human input), `done`, `idle`,
`unknown`. `wait agent-status --transition` arms only after the status
LEAVES the target first — use it right after sending a prompt, or an
already-idle pane resolves the wait instantly.

## Delegate and collect results

An orchestrator hands a pane a task and gets a structured result back —
no screen-scraping. Each pane holds ONE result slot, consumed on read.

```bash
"$CDOCK_BIN" agent start --profile orchestrator             # built-in coordinator role
# (users start one from the sidebar menu: "new orchestrator" — its tab keeps
#  an always-open team panel where they click chats in and out of the team;
#  ANY agent CLI can be the orchestrator — codex/agy/cline read the role
#  from AGENTS.md (.clinerules for cline) in their working folder, claude
#  from the prompt — and any mix of agents can be the workers: one system)
"$CDOCK_BIN" team list    # your workers WITH context: name, agent, status,
                          # workspace and its project folder (cwd) — read the
                          # folder directly; no snapshot/pane-read sweep needed
"$CDOCK_BIN" team set 7 "$CDOCK_PANE_ID"                    # adopt pane 7 into your team
"$CDOCK_BIN" agent start --profile reviewer --split right --team "$CDOCK_PANE_ID"
"$CDOCK_BIN" pane run 7 "full task prompt…"                 # paste + Enter, multiline-safe
"$CDOCK_BIN" wait task-result 7 --timeout 600000            # → {"ok":true,"result":"…"}
"$CDOCK_BIN" task result 7                                  # non-blocking fetch (also consumes)
"$CDOCK_BIN" task done "what I did and found"               # from INSIDE the worker pane
"$CDOCK_BIN" pane key 7 esc                                 # answer a TUI prompt (enter|esc|y|n|1..9|up|down)
"$CDOCK_BIN" pane read 7 --plain --lines 20                 # raw text, no JSON envelope
"$CDOCK_BIN" agent start --profile reviewer --wait-ready    # returns once it sits at its prompt
```

Rules: end every delegated prompt with the `task done …` instruction
(no --pid needed; sandboxed CLIs may have to escalate permissions for
the socket); read a result BEFORE closing its pane (one slot, read-once);
results cap at 256 KiB — summarize, don't dump; only WRITE into panes in
your team (`team list`) — others may belong to another orchestrator.
LOOKING is unrestricted: `pane list` / `api snapshot` / `pane read` any
pane whenever context helps.

An orchestrator's AGENT can be swapped in place from the team panel
("agent:" row → type codex/claude/…): same folder, memory, mode and
team; each agent's session ident is remembered in the folder, so
switching back offers restoring that conversation.

Orchestrator memory lives in its working folder (cdock memory, above the
CLI's own): STATE.md is the index — goal, team, statuses, links into
notes/ — updated continuously; a heavy context is handled by updating
STATE.md then self-running `/compact` (`pane run $CDOCK_PANE_ID
"/compact"`), and a relaunched or freshly started orchestrator recovers
from STATE.md even when the old conversation is gone.

Orchestrators are woken automatically: when a team member REPORTS a
result (`task done`) or turns blocked, cdock types "[cdock] team update
(mode: …): …" into the orchestrator's chat — assign work, end your
turn, and act on updates as they arrive. A worker merely going idle
never nudges (agents pause between turns mid-task) — but one idle
~10 minutes with nothing reported triggers a stall update: it may have
refused, crashed to its prompt, or finished silently; read its screen
and recover or re-task it. The mode inside the update is the orchestrator's CURRENT
policy — report (one final report), auto (goal-bound self-driving loop:
stop when the user's task is done, never invent side quests), notify
(ask the user each step); switch it in the team panel or with
`"$CDOCK_BIN" team mode <orch> report|auto|notify`.

While the USER edits a team pane (focused AND typing — merely looking
never blocks), writes into it are refused ("user is driving") and team
list shows `user_active: true` — don't retry; a handback update arrives
when they move on so you re-read the conversation first.

An orchestrator may grow its own team: spawn helpers into an existing
workspace (`agent start --profile <p> --workspace <id> --team
$CDOCK_PANE_ID` — ids in team list; your claude profile is inherited)
or adopt panes via `team set`. It may remove only members IT added —
`team list` marks each row `added_by: user|orchestrator`, and the server
refuses CLI removal of user-assigned ones.

## Rules

- NEVER test cdock server features against the user's live session. Spin a
  sandbox: `D=$(mktemp -d) && XDG_STATE_HOME=$D cdock --server` and point
  every test command at it with `XDG_STATE_HOME=$D`. A scoped attach or a
  workspace created in the default session gets autosaved over the user's
  real session within 5 seconds.
- Clean up panes you spawned when done: `"$CDOCK_BIN" pane focus <id>` the
  user can see it, or leave long-running watchers only if the user asked.
- Never `pane run` into a pane whose program you don't know — check
  `pane list` first; typing into another agent's pane injects text into
  their conversation.
- Pane ids are numbers; `%7` and `7` both work as arguments.

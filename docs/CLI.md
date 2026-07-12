# CLI

Every CLI command is a thin wrapper over the JSON socket API
(`api-<session>.sock`): it prints the server's JSON reply verbatim and
exits 0 when `ok` is true. `cdock api reference` (alias `schema`) prints
one example request per API command — the machine-readable catalog.

Pane ids accept `3` or `%3`.

## Default launch & top-level flags

```
cdock                      attach to the running server (or start one)
cdock --session NAME       use/create a named session (own server + snapshot)
cdock -f [PATH]            folder-scoped attach: only workspaces under PATH
cdock --no-session         everything in one process, ephemeral (no persistence)
cdock --config PATH        config file override (also CDOCK_CONFIG_PATH)
cdock --default-config     print the annotated default config
```

## `cdock pane <sub>`

```
list                         every pane: workspace/tab, program, agent, status
split [PANE] [--direction right|down] [--command CMD]
run PANE CMD                 write text + Enter
send-text PANE TEXT          write literal text (no Enter)
read PANE [--lines N]        last non-empty screen lines
focus PANE
observe PANE                 stream raw output to stdout until Ctrl-C
attach PANE                  two-way interactive attach; Ctrl-] detaches
report-agent PANE STATE [--label TEXT] [--ttl-ms N]
                             hook/wrapper-reported status: working|blocked|
                             done|idle|clear — overrides screen detection
report-metadata PANE --title TEXT
```

## `cdock agent <sub>`

```
list                         pane list filtered to recognized agents
start CMD | --profile NAME [--split right|down] [--workspace ID]
                             bare profile names prefer THIS workspace's
                             agents; ws:/global: pick a scope explicitly
behavior PANE IDENT|clear    inject a behavior profile (ws:<name> or
                             global:<name>) into the RUNNING session; the
                             role also rides into resume as system prompt
explain [PANE] | --file PATH --agent ID
                             full detection rule trace (why this status)
```

## `cdock session <sub>`

```
list                         name, running or not, snapshot size
attach NAME                  attach to (or start) a named session
attach ssh:HOST[/NAME]       exec ssh -t HOST cdock — remote attach
stop NAME                    save + stop the server (panes end)
delete NAME                  remove a stopped session's snapshot/leftovers
```

## `cdock workspace <sub>` / `cdock tab <sub>`

```
workspace focus ID | close ID
workspace create [--cwd PATH] [--ssh HOST]   --ssh runs `ssh -t HOST` in the pane
tab focus ID | create [--workspace ID] | close ID
```

## `cdock worktree <sub>`

```
list [--workspace ID]
create BRANCH [--workspace ID]    branch + worktree, opened as a child space
open BRANCH [--workspace ID]
remove --workspace ID [--force]
```

## `cdock wait <sub>`

```
wait output PANE --match TEXT [--timeout-ms N]
wait agent-status PANE --status working|blocked|done|idle [--timeout-ms N]
```

Exit 1 on timeout. `wait output` matches against a rolling tail of raw
output, so fast scrolling can't slip past the poll.

## `cdock server <sub>`

```
reload-manifests             re-read detection manifests (bundled + user)
reload-config                re-read config, keymap, theme
handoff                      exec the current binary in place — panes and
                             agents keep running, clients reconnect
```

## `cdock profile <sub>`

```
list                         ws:<name> entries for this cwd, then globals
show NAME                    resolved command + env (bare names: ws first)
new NAME [--from OTHER]      scaffold a global profile
new NAME --ws                scaffold a profile scoped to THIS workspace
                             (lives in cdock metadata, not the repo)
```

## `cdock skill <sub>`

```
list | add NAME --source DIR [--description TEXT] | remove NAME
```

## `cdock plugin <sub>`

```
list | link PATH | unlink ID
install gh:owner/repo        shallow clone + manifest validation
install PATH                 same as link
action PLUGIN ACTION         run a plugin action in the foreground
open ID                      open the panes a plugin declares under [[panes]]
```

Plugin manifests may declare `[[hooks]]` (`event = "blocked"|"done"|"status"`,
`run = "cmd"` — fired on agent status changes with `CDOCK_PANE`/`CDOCK_STATUS`)
and `[[panes]]` (`title`, `command`).

## `cdock events`

```
events [--pane ID] [--only agent-status,output]
```

NDJSON stream of server events until Ctrl-C.

## Misc

```
cdock api snapshot           full state tree as JSON
cdock api reference|schema   the socket API catalog
cdock integration install claude
                             SessionStart hook into EVERY ~/.claude* profile
cdock update [--handoff]     self-update; --handoff swaps the live server
```

## Environment inside panes

Every pane gets `CDOCK_BIN`, `CDOCK_PANE_ID`, `CDOCK_TAB_ID`,
`CDOCK_WORKSPACE_ID`, `CDOCK_SESSION`; profile launches add
`CDOCK_AGENT_PROFILE` and `CDOCK_AGENT_PROFILE_DIR`. Agents drive cdock
with `"$CDOCK_BIN" <command>` — see the bundled cdock skill.

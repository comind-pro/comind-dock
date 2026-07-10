# CLI Specification

Binary: `cdock`. One executable; the first argument selects the mode. Almost
every subcommand is a thin wrapper over the JSON socket API and prints JSON
where useful for scripting. Exit codes: 0 ok, 1 error, 2 usage.

## Default launch & top-level flags

- `cdock` — launch or attach to the persistent session (auto-starts the
  background server, then attaches a client).
- `-f, --folder [path]` — folder-scoped attach (default: current directory).
  The client shows only workspaces rooted at or under the folder — agents,
  history, and notifications for this project only. Creates a workspace with
  that cwd if none matches. Scope is per-client presentation state; the
  shared session and other clients are unaffected. Combinable with
  `--session`.
- `--session <name>` — use/create a named session.
- `--no-session` — monolithic single-process mode (no server/client split).
- `--remote <ssh-target>` — attach to a remote server over SSH
  (`ssh://user@host:port` accepted). Only valid with the default launch.
- `--remote-keybindings local|server` — whose keybindings apply during a
  remote attach (default local).
- `--handoff` — opt into live handoff during update or remote attach.
- `--default-config` — print the annotated default config and exit.
- `--version` / `-V`, `--help` / `-h`.

Hidden internal modes: `client` (connect to an existing server's client
socket), `server` (headless server), `remote-client-bridge` (pipes the binary
protocol over an SSH channel).

Nested-launch guard: refuses to start inside one of its own panes
(`CDOCK_ENV=1`) unless the experimental allow-nested setting is on.

## `cdock agent <sub>`

| Subcommand | Purpose |
|---|---|
| `list`, `get <target>`, `focus <target>` | Enumerate/inspect/focus agent panes |
| `read <target> [--source visible\|recent\|recent-unwrapped\|detection] [--lines N] [--format text\|ansi]` | Read an agent pane's output |
| `send <target> <text>` | Write literal text (no Enter) |
| `rename <target> <name>\|--clear` | Custom agent label |
| `wait <target> --status idle\|working\|blocked\|unknown [--timeout MS]` | Block until an agent reaches a status |
| `attach <target> [--takeover]` | Direct-attach the current terminal to the agent's pane |
| `start <name> [--cwd] [--workspace ID] [--tab ID] [--split right\|down] [--env K=V] [--focus\|--no-focus] -- <argv…>` | Spawn an agent pane |
| `start --profile <profile> [--cwd] [--workspace ID] [--tab ID] [--split …] [--env K=V]` | Spawn an agent from a profile: stages its skills and `agent.md`, then runs the profile's argv |
| `explain <target> [--json\|--verbose]`; `explain --file PATH --agent LABEL [--json]` | Dump detection reasoning: matched rule, region, priority, evidence, manifest source/version |

Targets accept terminal ids, unique agent names, and detected/reported labels.

## `cdock pane <sub>`

- Inspection: `list [--workspace ID]`, `current`, `get <id>`, `layout`,
  `process-info`, `neighbor --direction left|right|up|down`, `edges`.
- Control: `focus --direction …`, `resize --direction … --amount N`,
  `zoom [--toggle|--on|--off]`, `split <id> --direction right|down
  [--ratio F] [--cwd] [--env] [--focus|--no-focus]`,
  `swap --direction …` | `swap --source-pane ID --target-pane ID`,
  `move <id> --tab <tab> | --new-tab [--workspace] [--label] |
  --new-workspace [--label] [--tab-label]`, `close <id>`,
  `rename <id> <label>|--clear`.
- I/O: `read` (same flags as agent read, plus `--raw`), `send-text <id>
  <text>`, `send-keys <id> <key…>`, `run <id> <command>` (text + Enter).
- Agent reporting (used by integration hooks):
  `report-agent <id> --source ID --agent LABEL --state idle|working|blocked|
  unknown [--message] [--custom-status] [--seq N] [--agent-session-id]
  [--agent-session-path]`, `report-agent-session …`, `release-agent …`,
  `report-metadata <id> --source ID [--title|--clear-title] [--display-agent]
  [--custom-status] [--state-label STATUS=TEXT] [--ttl-ms N]`.

## `cdock workspace <sub>` / `cdock tab <sub>`

`list`, `create [--cwd] [--label] [--env] [--focus|--no-focus]
[--host <ssh-target>]`, `get`, `focus`, `rename`, `close`. Tab commands
additionally take `--workspace`. `create --host` makes a remote-backed
workspace: its panes spawn on that host via a remote bridge (see
`cdock remote`).

## `cdock remote <sub>`

Per-workspace remote bridges — mix local and remote work in one session
(contrast with `--remote`, which attaches the whole client to a remote
server).

- `mount <host> [--session <name>] [--label]` — bridge a remote server's
  workspaces into the local session; bootstraps the remote server and binary
  like `--remote` does.
- `unmount <host|bridge-id>` — drop the bridge; remote agents keep running on
  their server.
- `list [--json]` — active bridges and connection state.

## `cdock worktree <sub>`

`list`, `create [--branch] [--base] [--path] [--label]`,
`open (--path|--branch)`, `remove --workspace ID [--force]`.
All accept `--workspace ID | --cwd PATH` and `--json`.

## `cdock wait <sub>`

- `output <pane> --match <text> [--regex] [--source …] [--lines] [--timeout
  MS] [--raw]` — exit 1 on timeout.
- `agent-status <pane> --status … [--timeout MS]`.

## `cdock session <sub>`

`list [--json]`, `attach <name>`, `stop <name> [--json]`,
`delete <name> [--json]`.

## `cdock terminal <sub>`

- `attach <terminal_id> [--takeover]` — direct attach (detach with
  `prefix q`).
- `session observe <target> [--cols] [--rows]` — read-only JSON frame stream.
- `session control <target> [--takeover] [--cols N] [--rows N]` — writable
  frame stream accepting JSON input/resize/scroll/release commands.
- `title set <title>` / `title clear` — outer terminal window title.

## `cdock server <sub>`

- bare `server` — run the headless server in the foreground (for supervised
  setups).
- `stop` — end the session and its panes.
- `reload-config` — apply config changes to the running server.
- `agent-manifests [--json]` — list loaded detection manifests.
- `update-agent-manifests [--json]` — fetch remote manifests and reload.
- `reload-agent-manifests` — reload from disk (picks up local overrides).
- `live-handoff [--import-exe] [--expected-protocol] [--expected-version]` —
  hand live panes to a replacement server.

## `cdock plugin <sub>`

`install <owner>/<repo>[/subdir] [--ref REF] [--yes]`,
`uninstall <id|owner/repo>`, `link <path> [--disabled]`, `unlink <id>`,
`list [--plugin ID] [--json]`, `enable <id>` / `disable <id>`,
`config-dir <id>`, `action list|invoke <action_id> [--plugin]`,
`log list [--plugin] [--limit]`,
`pane open --plugin --entrypoint [--placement overlay|split|tab|zoomed]
[--workspace] [--target-pane] [--direction] [--cwd] [--env] [--focus]`,
`pane focus|close`.

## `cdock profile <sub>`

Agent profiles (role + skills + memory; see FEATURES §21).

- `list [--json]` — all profiles.
- `show <name> [--json]` — resolved profile: argv, skills, files, flags.
- `new <name> [--from <profile>]` — scaffold a profile directory
  (`profile.toml`, `agent.md`, optional `memory.md`).

Profiles are plain files under `~/.config/comind-dock/agents/<name>/`; edit
them directly.

## `cdock skill <sub>`

Explicit skill catalog (see FEATURES §21).

- `list [--json]` — catalog with sources and versions.
- `add <name> --source <path|git-ref> [--version]` — register a skill.
- `remove <name>` — unregister (profiles referencing it get a startup
  warning).

## `cdock integration <sub>`

`install <agent>`, `uninstall <agent>`, `status [--outdated-only]`.
Targets: the supported coding-agent CLIs (see FEATURES §11).

## Misc commands

| Command | Purpose |
|---|---|
| `cdock channel show` / `set stable\|preview` | Update channel; writes config, then self-updates or prints package-manager guidance |
| `cdock config reset-keys` | Back up config and strip key sections to restore built-in bindings |
| `cdock api snapshot` | One-time full runtime state snapshot (JSON) |
| `cdock api schema [--json] [--output PATH]` | Print the socket API JSON Schema |
| `cdock notification show <title> [--body] [--position top-left\|top-right\|bottom-left\|bottom-right] [--sound none\|done\|request]` | Show a toast |
| `cdock status [server\|client] [--json]` | Client/server status, protocol compat, socket path |
| `cdock completion <bash\|zsh\|fish\|powershell\|elvish>` | Shell completions |
| `cdock update [--handoff]` | Self-update from the channel feed |

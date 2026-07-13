# Configuration

Format: TOML. Zero-config startup works; every setting has a default.
`cdock --default-config` prints the fully annotated default file — it is
the authoritative reference; this page explains the semantics. A broken
file falls back per SECTION: an invalid section gets its defaults and a
boot toast names it, the rest of the file still applies. `cdock server
reload-config` applies changes to a running server.

## Location

Priority: `--config PATH` > `CDOCK_CONFIG_PATH` >
`$XDG_CONFIG_HOME/comind-dock/config.toml` (default
`~/.config/comind-dock/config.toml`). State (logs, session snapshots,
screen history) lives in `$XDG_STATE_HOME/comind-dock`
(`~/.local/state/comind-dock`).

## Sections

### `[theme]`
- `name` — `default` | `terminal` (follow the host ANSI palette).
- `[theme.custom]` — per-token overrides: `#rrggbb`, `rgb(r,g,b)`, named
  colors, `reset`. Tokens include `accent`, `divider`, `tab_bar_bg`,
  `muted`, …

### `[terminal]`
- `default_shell` — executable for new panes (empty → `$SHELL` → `/bin/sh`).
- `shell_mode` — `auto | login | non_login` (auto: login shells on macOS
  for PATH setup).
- `new_cwd` — `follow | home | current | <fixed path>`; `follow` spawns
  where the focused pane's process currently sits.
- `editor` — editor for settings/profiles/skills opened from the UI
  (empty → `$EDITOR` → `$VISUAL` → `nano`). Also settable from the app
  menu (`editor…`), which persists the choice here without touching your
  comments.

### `[keys]`
- `prefix` (default `ctrl+b`).
- One key (or an array) per action: bare key = after the prefix, a chord
  with modifiers (`"ctrl+alt+t"`) binds directly. Actions: `split_right`,
  `split_down`, `focus_left/down/up/right`, `swap_*`, `resize_mode`,
  `zoom`, `close_pane`, `new_tab`, `next_tab`, `prev_tab`, `rename_tab`,
  `close_tab`, `new_workspace`, `rename_workspace`, `close_workspace`,
  `next_workspace`, `toggle_sidebar`, `help`, `quit`. `prefix+1..9` jumps
  to tab 1..9.
- `[[keys.command]]` — custom bindings: `key`, `type = "pane" | "shell"`,
  `command`, optional `description` (shown in the help overlay).

### `[ui]`
- `sidebar_width` (24), `mouse_capture` (true), `mouse_scroll_lines` (3),
  `confirm_close`, `prompt_new_tab_name`, `hide_tab_bar_when_single_tab`.
- `[ui.sound]` — `enabled` (true): terminal bell + system sound on agent
  blocked/finished; `CDOCK_DISABLE_SOUND` env kills all sounds.
- `[ui.toast]` — `delivery = app | system | both | off`; app toasts are
  clickable (jump to the pane). Suppressed for the pane you are looking at.

### `[worktrees]`
- `directory` (default `~/.comind-dock/worktrees`) — checkout root, laid
  out as `<dir>/<repo>/<branch-slug>`.

### `[update]`
- `check` (true) — background release check on boot and every 6h; the app
  menu shows "● update ready". Config reloads are picked up by the checker.
- `repo` — the GitHub repo the feed reads from.
- `channel` — `stable` (full releases) | `preview` (prereleases included).

### `[restore]`
- `screen_history` (false) — save pane screen tails (last 200 lines per
  pane) to the state dir and replay them after a full server restart.
  Off by default: raw terminal output may contain secrets. Turning it
  off also purges previously stored tails on the next autosave. Live
  handoff replays screens regardless — its tail files are one-shot and
  deleted the moment the new server adopts the panes.

### `[advanced]`
- `scrollback_limit_bytes` (10_000_000) — converted to emulator lines
  (~4 KB per stored line at typical widths, clamped to 1k–10k lines).

### `[experimental]`
- `allow_nested` — permit launching cdock inside a managed pane.

## Agent profiles

Not part of `config.toml` — each profile is a directory of plain files:

```
~/.config/comind-dock/agents/<name>/              # global
~/.config/comind-dock/workspaces/<cwd-slug>/agents/<name>/   # workspace-scoped
├── profile.toml   # command, [env], skills = ["..."], orchestrator = bool
├── agent.md       # the role — staged in as system prompt (claude adapter)
└── memory.md      # optional persistent per-role memory
```

Workspace-scoped profiles are keyed to a space's folder but live in cdock
metadata, not the repo. Create either kind from the UI (menu → agents…) or
the CLI (`cdock profile new NAME [--ws]`). Launching stages
`staged-prompt.md` (role + memory + assigned skills + orchestrator roster)
and sets `CDOCK_AGENT_PROFILE`/`CDOCK_AGENT_PROFILE_DIR` in the pane.

## Skills

`~/.config/comind-dock/skills.toml` — machine-managed catalog mapping a
name to a directory containing `SKILL.md` (+ description). Managed from
the UI (menu → skills…) or `cdock skill add/remove`; assign to a profile
from the profile's menu (skills…) or `skills = [...]` in profile.toml.

## Agent status: hooks first, screen second

`cdock integration install claude` writes hooks into every `~/.claude*`
profile so claude reports its own lifecycle: `UserPromptSubmit`/`PreToolUse`/
`PostToolUse` → working, `Notification` → **blocked** (it wants you),
`Stop` → done, `SessionEnd` → clear. A reported state outranks screen
detection until its TTL expires, so a claude release that renames a spinner
can no longer freeze your statuses. The hooks are guarded: they do nothing
outside a cdock pane, and the server rejects a report whose pid is not the
pane's agent (a nested claude cannot speak for its parent).

Any wrapper can do the same: `cdock pane report-agent <pane> working
--label "running tests" --ttl-ms 60000`.

## Detection manifests

`src/detect/manifests/*.toml` bundled; user overrides in
`~/.config/comind-dock/manifests/`. `cdock server reload-manifests`
re-reads them; `cdock agent explain` shows the full rule trace.

## Environment variables

### User-facing
| Variable | Purpose |
|---|---|
| `CDOCK_CONFIG_PATH` | Config file override |
| `CDOCK_SESSION` | Named session (also `--session`) |
| `CDOCK_LOG` | Log filter |
| `CDOCK_DISABLE_SOUND` | Kill switch for all sounds |
| `CDOCK_DEV` | Isolated dev namespace (also the `cdock-dev` argv0 symlink) |
| `CDOCK_INSTALL_DIR` | install.sh target (default `~/.local/bin`) |
| `GITHUB_TOKEN` | Authenticated update checks (else `gh auth token`) |

### Injected into panes
`CDOCK_BIN`, `CDOCK_PANE_ID`, `CDOCK_TAB_ID`, `CDOCK_WORKSPACE_ID`,
`CDOCK_SESSION`; profile launches add `CDOCK_AGENT_PROFILE` and
`CDOCK_AGENT_PROFILE_DIR`; claude panes get the SessionStart hook wired by
`cdock integration install claude`.

### Injected into plugin hooks
`CDOCK_PANE`, `CDOCK_STATUS` (agent status-change hooks).

## Logs

Rotating log files in the state directory; filter with `CDOCK_LOG`.

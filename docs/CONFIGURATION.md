# Configuration

Format: TOML. Zero-config startup works; every setting has a default.
`cdock --default-config` prints the fully annotated default file. Invalid
values fall back to defaults with startup warnings. `cdock server
reload-config` applies changes to a running server (UI settings apply without
restarting panes) and produces a diagnostics report.

## Location

- Override: `CDOCK_CONFIG_PATH` points directly at the config file.
- Otherwise: `$XDG_CONFIG_HOME/comind-dock/config.toml`, falling back to
  `~/.config/comind-dock/config.toml` (Unix); `%APPDATA%\comind-dock\
  config.toml` (Windows). State (logs, session snapshots) goes to the
  matching XDG state directory.

## Sections

### Top level
- `onboarding` (bool) — first-run guided setup.

### `[theme]`
- `name` — built-in theme name; `terminal` follows the host ANSI palette.
- `auto_switch`, `light_name`, `dark_name` — follow host light/dark
  appearance changes.
- `[theme.custom]` — per-token color overrides: hex, named, `rgb()`, and
  `reset`/`transparent` aliases (e.g. panel background inheriting terminal
  transparency).

### `[terminal]`
- `default_shell` — executable for new panes (empty → `$SHELL` → platform
  default).
- `shell_mode` — `auto | login | non_login` (auto uses login shells on macOS
  for PATH setup).
- `new_cwd` — `follow | home | current | <fixed path>`.

### `[session]`
- `resume_agents_on_restore` (bool, default on) — relaunch supported agents
  into their native conversations after a server restart.

### `[update]`
- `channel` — `stable | preview`.
- `version_check` (bool) — background new-version checks.
- `manifest_check` (bool) — background agent-detection manifest updates.

### `[keys]`
- `prefix` (default `ctrl+b`).
- One key (or array of keys) per action: help, settings, detach,
  reload-config, workspace/tab/pane creation, navigation, rename, close,
  splits, zoom, resize mode, sidebar toggle, session navigator, copy mode,
  scrollback editor, worktree actions, agent cycling, navigate-mode movement.
- Binding syntax: plain keys, `ctrl/shift/alt/cmd/super` modifiers, explicit
  `prefix+X` vs direct chords, named punctuation, `1..9` index ranges for
  indexed families (switch tab / switch workspace / focus agent).
- `[[keys.command]]` — custom bindings: `key`, `type = "pane" | "shell" |
  "plugin_action"`, `command`, optional `description` (shown in the help
  overlay). Commands receive runtime context env vars.
- `remote_image_paste` (default `ctrl+v`) — active only during remote attach.
- `cdock config reset-keys` restores built-in bindings with a timestamped
  backup.

### `[ui]`
- Sidebar: `sidebar_width`, min/max bounds, `sidebar_collapsed_mode =
  compact | hidden`.
- `mobile_width_threshold` (default 64) — single-column narrow layout.
- Mouse: `mouse_capture` (bool), `mouse_scroll_lines` (default 3),
  `right_click_passthrough_modifier`.
- Panes: `pane_borders`, `pane_gaps`, `accent`,
  `show_agent_labels_on_pane_borders`, `confirm_close`,
  `prompt_new_tab_name`, `hide_tab_bar_when_single_tab`.
- `agent_panel_sort = spaces | priority`.
- `host_cursor = auto | native | drawn`; `redraw_on_focus_gained`.
- `[ui.toast]` — `delivery = off | app | terminal | system`,
  `delay_seconds` (notify only if the state persists; suppressed for the
  active tab). `[ui.toast.app]` position; `[ui.toast.clipboard]`
  copy-confirmation popup.
- `[ui.sound]` — `enabled` (default on), custom audio `path` /
  `done_path` / `request_path`, `[ui.sound.agents]` per-agent
  `default | on | off`.

### `[worktrees]`
- `directory` (default `~/.comind-dock/worktrees`) — checkout root, laid out
  as `<dir>/<repo>/<branch-slug>`.

### `[advanced]`
- `scrollback_limit_bytes` (default 10_000_000).

### `[experimental]`
- `allow_nested` — permit launching inside a managed pane.
- `kitty_graphics` — inline image rendering.
- `pane_history` — persist recent pane screen contents across restarts (off
  by default; output may contain secrets).
- macOS IME helpers: `switch_ascii_input_source_in_prefix`,
  `reveal_hidden_cursor_for_cjk_ime`, `cjk_ime_agents` (allow-list),
  `cjk_ime_cursor_shape`.

### `[remote]`
- `manage_ssh_config` — wrap ssh with a generated config adding keepalives
  and a per-attach control socket for connection reuse.

### `[skills]`
Explicit skill catalog (see FEATURES §21).
- `directory` (default `~/.config/comind-dock/skills`) — where fetched skills
  are stored.
- `[skills.catalog.<name>]` — one entry per skill: `source` (local path or
  git ref), optional `version`, `description`.

## Agent profiles

Not part of `config.toml` — each profile is a directory under
`~/.config/comind-dock/agents/<name>/`:

```
agents/<name>/
├── profile.toml   # argv, model, env, cwd policy, skills = [..],
│                  # workspaces = [..], orchestrator = bool, memory = bool
├── agent.md       # role definition, injected at launch
└── memory.md      # optional persistent per-role memory
```

Launching a profile stages its skills and `agent.md` into the target agent
CLI's expected locations and sets `CDOCK_AGENT_PROFILE_DIR` in the pane
environment. Plain files by design: edit, diff, commit to dotfiles — or use
the built-in profile & skill editor (sidebar button), which reads and writes
the same files. `workspaces` associates the profile with workspaces acting as
its categories; those workspaces surface it first in their new-pane pickers.

## Environment variables

### User-facing
| Variable | Purpose |
|---|---|
| `CDOCK_CONFIG_PATH` | Config file override |
| `CDOCK_SESSION` | Named session |
| `CDOCK_SOCKET_PATH` | JSON API socket override |
| `CDOCK_LOG` | Log filter |
| `CDOCK_DISABLE_SOUND` | Kill switch for all sounds |
| `CDOCK_AGENT` | Detection-manifest hint when wrappers hide the real process |
| `CDOCK_REMOTE_BINARY` | Path to a local build for remote bootstrap |
| `CDOCK_INSTALL_DIR` | Install script target (default `~/.local/bin`) |

### Injected into panes
`CDOCK_ENV=1`, `CDOCK_PANE_ID`, `CDOCK_TAB_ID`, `CDOCK_WORKSPACE_ID`,
`CDOCK_SOCKET_PATH`, `CDOCK_BIN_PATH`; profile-launched panes additionally
get `CDOCK_AGENT_PROFILE_DIR`.

### Injected into plugin processes
`CDOCK_PLUGIN_ID`, `CDOCK_PLUGIN_ROOT`, `CDOCK_PLUGIN_CONFIG_DIR`,
`CDOCK_PLUGIN_STATE_DIR`, `CDOCK_CONTEXT_JSON`, `CDOCK_ACTION_ID`,
`CDOCK_EVENT`, `CDOCK_EVENT_JSON`, `CDOCK_ENTRYPOINT_ID`,
`CDOCK_CLICKED_URL`, `CDOCK_LINK_HANDLER_ID` — plus socket/bin paths and
current ids.

### Injected into integration hooks
`CDOCK_INTEGRATION_ID`, `CDOCK_INTEGRATION_VERSION`, hook input payload
variables.

## Logs

Capped, rotating per-mode log files in the state directory: combined, client,
and server logs. Filtering via `CDOCK_LOG`.

# Feature Catalog

> Written as the design catalog; as of v0.4.x it is implemented, with
> these exceptions: Windows/ConPTY, kitty graphics, IME composition
> (deferred — see docs/ROADMAP.md), and a few spec details that shipped
> in simplified form (marked `ponytail:` in the code).

Complete user-facing feature set for comind-dock. Keybindings below assume the
default prefix `ctrl+b` (written `prefix`); every binding is configurable.
Nearly every action is reachable three ways: keyboard, mouse, and the `cdock`
CLI (which wraps the JSON socket API).

## 1. Workspaces, Tabs, Panes

The organizational model: **workspace → tab → pane**.

- **Workspaces** — top-level containers, one per repo/task/investigation. Own
  tabs and panes; agent state rolls up to the workspace in the sidebar.
  CLI: `cdock workspace create|focus|rename|close|list|get`. Keys: new
  `prefix+shift+n`, rename `prefix+shift+w`, close `prefix+shift+d`, picker
  `prefix+w`. `create` supports `--cwd`, `--label`, `--env`, `--focus/--no-focus`.
- **Tabs** — layout subcontexts inside a workspace (agents / logs / server /
  review). CLI: `cdock tab create|focus|rename|close|list|get`. Keys: new
  `prefix+c`, next/prev `prefix+n`/`prefix+p`, jump `prefix+1..9`, rename
  `prefix+shift+t`, close `prefix+shift+x`.
- **Panes** — real terminals, each running its own process, rendered by the
  runtime and surviving client detach.
- **Splits** — split right or down with an optional ratio. Keys `prefix+v`
  (right), `prefix+-` (down); `cdock pane split --direction right|down
  [--ratio F]`; right-click menu; drag borders with the mouse to resize.
- **Focus & navigation** — directional focus `prefix+h/j/k/l`; cycle
  `prefix+tab` / `prefix+shift+tab`; optional last-pane toggle. CLI:
  `cdock pane focus --direction`, `pane neighbor`, `pane edges`.
- **Resize mode** — `prefix+r`, or `cdock pane resize --direction --amount`.
- **Zoom** — temporarily maximize the focused pane within its tab; zoomed tabs
  are marked in the tab bar. Key `prefix+z`; `cdock pane zoom
  [--toggle|--on|--off]`.
- **Swap** — swap two panes directionally or by explicit ids, preserving split
  shape and ratios. Keys `prefix+shift+h/j/k/l`; `cdock pane swap`; context menu.
- **Move** — relocate a running pane into another tab, a new tab, or a new
  workspace without restarting its process. `cdock pane move --tab|--new-tab|
  --new-workspace`.
- **Rename / labels** — manual pane labels (`cdock pane rename <id> <label>|
  --clear`); optional detected-agent labels on split borders.
- **Close** — `cdock pane close`; key `prefix+x`; optional confirm dialog.
- **Inspection** — `cdock pane list|get|current|layout|process-info` return
  JSON: layout tree, split ratios, foreground process pid/argv/cwd, scroll
  metrics.
- **Layout export/apply** — export a tab's binary split tree and re-apply it
  declaratively (structure, labels, cwd, env, optional startup commands).
- **Sidebar** — the main dashboard of workspaces/tabs/panes/agents. Toggle
  `prefix+b`; collapsed mode compact or hidden.
- **Session navigator** — searchable workspace/tab/pane tree with agent-state
  filters, mouse and keyboard navigation. Key `prefix+g`.
- **Folder-scoped attach** — `cdock -f [path]` (default: current directory)
  attaches a client scoped to that folder: the sidebar, agent panel,
  navigator, and notifications show only workspaces rooted at or under the
  folder, so one project's agents don't drown in the global list. If no
  matching workspace exists, one is created with that cwd. The scope is a
  client-side view over the shared session — everything else keeps running
  and other clients are unaffected; a toggle (`prefix+shift+f` or a sidebar
  control) widens the view to all workspaces without reattaching.
- **New-tab naming prompt** — optionally prompt for a label when creating tabs.

## 2. Agent Detection, Status, Control

- **Automatic detection** — recognizes coding agents from the foreground
  process, screen-content manifests, and optional integration hooks. Detection
  rules ship per agent and cover the popular agent CLIs; new agents are added
  by writing a manifest, not code.
- **Status model** — `blocked` (needs input/approval), `working`, `done`
  (finished, not yet seen), `idle` (finished, seen), `unknown`.
- **Status authority** — one authority per pane: a full-lifecycle integration
  hook authors state directly; otherwise screen-manifest detection classifies
  a snapshot of the pane's bottom buffer (deliberately strict for `blocked`).
- **State rollups** — pane → tab → workspace inherit the most attention-worthy
  state so the sidebar surfaces what needs you.
- **Agent panel** — all agents across all workspaces, sorted by location or by
  priority (blocked → done → working → idle → unknown). Cycle bindings and
  indexed focus `prefix+alt+1..9`.
- **Detection manifests** — TOML manifests bundled with the binary,
  auto-updated from a remote catalog (can be disabled), with local overrides
  that always win. Refresh/reload via `cdock server update-agent-manifests` /
  `reload-agent-manifests`.
- **Explain (debug)** — `cdock agent explain <target>` shows why a state was
  chosen: matched rule, evidence, manifest source and version, skip reasons.
  Also works offline against a captured screen file.
- **Sandbox hint** — `CDOCK_AGENT=<agent>` names the manifest to use when
  wrappers (VMs, sandboxes) hide the real process.
- **Custom labels & display metadata** — rename agents; integrations can add
  visual-only status labels (title, display name, per-state labels, TTL)
  without changing semantic state.
- **Visual progress** — working agents surface progress and color accents on
  their pane border and sidebar entry (fed by OSC progress reports and
  detection state), so activity is visible at a glance.
- **Agent CLI** — `cdock agent start|read|send|focus|wait|list|get|rename|
  attach|explain`; targets accept terminal ids, agent names, detected labels.

## 3. Sessions, Persistence, Restore

- **Persistent server + thin client** — the default model: a background server
  owns panes and processes; clients attach, render, detach. Multiple clients
  can attach to the same session simultaneously.
- **Detach / reattach** — detach with `prefix+q` or by closing the terminal;
  agents keep running. Reattach by running `cdock` again.
- **Named sessions** — independent server namespaces (own panes, sockets,
  state; shared config). `cdock session list|attach|stop|delete`; env
  `CDOCK_SESSION`.
- **Snapshot restore** — after a full server restart, workspace/tab/pane
  shape, cwd, layout, and focus are restored (processes restart as fresh
  shells).
- **Pane screen history replay** — opt-in restore of recent pane contents
  across restarts; off by default because output may contain secrets.
- **Native agent session resume** — after a restart, supported agents are
  relaunched into their own native conversation using integration-reported
  session references (on by default, configurable).
- **Live handoff** — opt-in transfer of live panes to a replacement server
  process, keeping agent processes alive across updates and remote
  re-bootstrap (`cdock update --handoff`).
- **Single-process escape hatch** — `cdock --no-session` runs without the
  server/client split (debugging, compatibility).
- **Server control** — `cdock server` (explicit headless run), `cdock server
  stop`.

## 4. Remote Access

- **SSH-in** — SSH to the host and run `cdock` there; the TUI adapts to
  narrow screens (works from phones/tablets via any SSH client).
- **Remote thin-client attach** — `cdock --remote <host>` streams a remote
  session to the local client, bootstrapping the remote server and
  auto-installing a matching remote binary when needed.
- **Mixed local/remote workspaces** — remote is per-workspace, not
  all-or-nothing. `cdock remote mount <host> [--session <name>]` bridges a
  remote server's workspaces into the local session; `cdock workspace create
  --host <ssh-target>` creates a workspace whose panes spawn on that host.
  One sidebar, one agent panel, one notification stream across local and
  remote work; remote workspaces carry a host badge. The remote server owns
  its agents — a dropped connection marks the workspace disconnected and
  auto-reconnects with keepalive, agents keep running. `cdock remote
  unmount|list` manage bridges.
- **Remote keybindings choice** — `--remote-keybindings local|server`
  (local muscle memory by default).
- **Remote named sessions** — `cdock --remote <host> --session <name>`.
- **Managed SSH config** — optional generated SSH config with keepalives and a
  per-attach control socket for connection reuse (one auth prompt).
- **Custom remote binary** — `CDOCK_REMOTE_BINARY=<path>` for local builds.
- **Clipboard image bridging** — paste local clipboard images into remote
  panes (remote-attach mode only).

## 5. Direct Terminal Attach & Bridges

- **Direct attach** — attach the current terminal to a single server-owned
  terminal instead of the full UI: `cdock agent attach <target>` /
  `cdock terminal attach <id>` (`--takeover`). Detach `prefix q`; literal
  prefix via double-prefix; scrollback via wheel/PageUp/PageDown.
- **Read-only observer** — `cdock terminal session observe <target>` streams
  newline-delimited JSON ANSI frames for third-party bridges; multiple
  observers allowed.
- **Writable controller** — `cdock terminal session control <target>` streams
  frames and accepts JSON input/resize/scroll/release commands (single owner,
  takeover supported).
- **Outer window title** — `cdock terminal title set|clear`.

## 6. Automation Primitives

- **Read pane output** — `cdock pane read <id> --source visible|recent|
  recent-unwrapped|detection [--lines N] [--format text|ansi]`. Sources:
  visible viewport, recent scrollback (wrapped/unwrapped), detection
  bottom-buffer snapshot.
- **Send input** — `cdock pane send-text` (no Enter), `pane send-keys`
  (key-combo syntax), `pane run` (text + Enter atomically).
- **Wait on output** — `cdock wait output <id> --match <text> [--regex]
  [--timeout MS]` (nonzero exit on timeout).
- **Wait on agent status** — `cdock wait agent-status <id> --status ...
  [--timeout MS]`.
- **Event subscriptions** — long-lived event streams for workspace/tab/pane/
  layout/worktree/agent lifecycle; one-shot conditional waits.

## 7. Socket API & CLI Surface

- **JSON socket API** — newline-delimited JSON over a Unix domain socket
  (named pipe on Windows). Dot-notation methods across server/session/
  workspace/worktree/tab/pane/layout/agent/events/integration/plugin/
  notification/client areas.
- **CLI = API wrapper** — the whole `cdock` CLI wraps the socket; most
  commands print JSON for scripting.
- **Bootstrap snapshot** — one-time full-state snapshot for clients keeping
  local caches (`cdock api snapshot`).
- **Schema introspection** — `cdock api schema` prints the bundled protocol
  JSON Schema.
- **Status** — `cdock status [server|client]`: protocol compatibility, socket
  path, restart-needed.
- **Shell completions** — bash, zsh, fish, powershell, elvish.
- **Pane environment** — panes get `CDOCK_ENV=1`, `CDOCK_PANE_ID`,
  `CDOCK_TAB_ID`, `CDOCK_WORKSPACE_ID`, `CDOCK_SOCKET_PATH`, `CDOCK_BIN_PATH`.
- **Protocol versioning** — the server carries a protocol version; an
  incompatible client/server pair prompts stop/restart guidance.

## 8. Agent Self-Use

- **Agent skill file** — a reusable instruction file teaching a coding agent
  running inside a pane (gated on `CDOCK_ENV=1`) to drive the runtime: spawn
  panes, read siblings' output, run servers/tests in parallel panes, wait on
  output or on other agents' status, spawn more agents.
- **Skill distribution** — installable with a one-line skills/package-manager
  command (per-project or global) or by pasting into the agent's instruction
  file.
- **Human onboarding guide** — a companion document an agent can use to walk a
  human through setup.

## 9. Git Worktrees

- **Worktree groups** — create git worktree checkouts as grouped child
  workspaces under the source repo. Sidebar actions plus CLI
  `cdock worktree list|create|open|remove` (`--branch --base --path --label
  --force`). Key `prefix+shift+g` for new worktree.
- **Checkout root** — configurable directory, laid out as
  `<dir>/<repo>/<branch-slug>`.
- **Safe deletion** — removal runs the git-native worktree removal (safe, then
  forced with confirmation); branches are never deleted; closing the parent
  group never deletes folders or branches.

## 10. Plugins

- **Local executable plugins** — a manifest file plus out-of-process argv
  commands in any language; plugins declare actions, event hooks, terminal
  panes, and link handlers. The whole CLI is the plugin API (via
  `CDOCK_BIN_PATH`).
- **Install / manage** — `cdock plugin install <owner>/<repo>[/subdir]
  [--ref --yes]` (GitHub shorthand, trust preview, optional build commands),
  `uninstall|list|enable|disable`, local dev `link|unlink`, `config-dir`.
- **Actions** — `cdock plugin action list|invoke`; bindable to keys.
- **Managed plugin panes** — `cdock plugin pane open --plugin --entrypoint
  [--placement overlay|split|tab|zoomed]`, `pane focus|close`.
- **Event hooks** — run plugin commands on runtime events (e.g. worktree
  created).
- **Link handlers** — route ctrl-click on matching terminal URLs (regex) to a
  plugin action instead of the browser.
- **Injected context** — `CDOCK_PLUGIN_ID/ROOT/CONFIG_DIR/STATE_DIR/
  CONTEXT_JSON/ACTION_ID/EVENT/ENTRYPOINT_ID` plus socket/bin paths and
  current ids.
- **Marketplace** — an auto-generated index of public GitHub repos tagged with
  a designated topic, refreshed periodically; searchable and sortable.
  Listing is automatic and unreviewed; trust guidance applies.
- **Command logs** — `cdock plugin log list`.

## 11. Agent Integrations

- **Install / uninstall / status** — `cdock integration install|uninstall
  <agent>`, `integration status [--outdated-only]`; also from the Settings
  integrations tab, which recommends agents found on PATH.
- **Two roles** — *lifecycle authority* integrations author agent state
  directly from the agent's own hook events; *session identity* integrations
  report native session references for restore while state stays
  screen-detected.
- **Per-agent installs** — each integration writes a hook/plugin/extension
  into that agent's own config directory (respecting the agent's env
  overrides) and can be cleanly uninstalled. Versioned with integration
  version markers so outdated hooks are flagged.
- **Custom integrations** — third parties report state through the socket API
  (report-agent / report-agent-session / report-metadata).

## 12. Updates & Release Channels

- **Self-update** — `cdock update` for script-managed installs; package-manager
  installs (Homebrew/Nix/mise) get the right upgrade command instead.
  `--handoff` keeps live panes alive across the update.
- **Channels** — `cdock channel show|set stable|preview`; stable is the
  default, preview ships prerelease builds from the main branch.
- **Background checks** — periodic version check and detection-manifest check
  (both individually configurable); in-app update badges when outdated.
- **First-run onboarding** — guided setup that opens the integrations tab.

## 13. Installation

- Shell install script (curl | sh), PowerShell script on Windows, Homebrew,
  mise, Nix flake (run/build/profile install, dev shell), and manual
  per-platform binaries. Release assets per target:
  `linux-x86_64`, `linux-aarch64`, `macos-x86_64`, `macos-aarch64`
  (+ `windows-x86_64` on preview).

## 14. Keyboard & Input Model

- **Prefix mode** — a single reserved, configurable prefix (`ctrl+b` default)
  followed by an action key; `prefix+?` lists all active bindings;
  double-prefix sends a literal prefix byte to the pane.
- **Key hints** — a status-bar strip at the bottom of the screen showing the
  most relevant key chords for the current mode, so common actions are
  discoverable without opening the full help overlay.
- **Three input modes** — terminal (keys go to the pane), prefix (one action),
  navigate (persistent workspace-nav surface with plain-key `h/j/k/l`
  movement).
- **Fully configurable bindings** — explicit `prefix+…` vs direct-chord
  syntax, multiple bindings per action, indexed `1..9` families (switch tab /
  workspace / focus agent), prefix-free chord recommendations. A reset command
  restores defaults with a timestamped backup.
- **Custom command keybindings** — user-defined bindings of type pane / shell /
  plugin-action, with optional descriptions shown in the help overlay,
  receiving runtime context env vars.
- **Copy mode** — `prefix+[`: vim-like motions, visual selection, yank,
  smart-case search with `n`/`N`. Mouse drag-select copies without entering
  copy mode.
- **Scrollback editor** — optional binding opens retained scrollback in
  `$EDITOR` in a temporary zoomed pane.
- **IME support** — optional macOS ASCII input-source switching during prefix
  mode; optional CJK IME cursor reveal for TUIs that paint their own cursor.

## 15. Mouse-Native UI

- Click panes/tabs/workspaces/agents to focus; drag split borders to resize;
  right-click context menus (split, new tab, swap, close); drag-select to
  copy; double-click a token to copy it; scroll over the tab bar to switch
  tabs; drag to reorder tabs.
- **Mouse capture toggle** — return clicks to the host terminal while still
  forwarding to pane apps that request mouse.
- **Scroll tuning** — lines per wheel notch; PageUp/PageDown scroll pane
  scrollback for primary-screen apps; alternate-screen apps get wheel events
  directly.
- **Ctrl-click links** — opens OSC 8 hyperlinks and visible URLs; modifier
  variants bypass to the host terminal.
- **Right-click passthrough** — configurable modifier forwards right-click
  hold/drag to mouse-reporting pane apps.

## 16. Theming & Appearance

- **Built-in themes** — a set of popular palettes with light/dark variants,
  plus a `terminal` theme that follows the host ANSI palette.
- **Auto light/dark** — follow host terminal appearance changes, with separate
  light and dark theme names.
- **Custom colors** — per-token overrides (hex, named, rgb(), reset/
  transparent aliases); panel background can inherit terminal transparency.
- **Pane chrome** — borders, gaps, accent color, sidebar width bounds.
- **Inline images** — experimental kitty-graphics-protocol rendering.
- **Host cursor** — auto/native/drawn rendering (drawn default on Windows to
  avoid flicker); optional redraw on focus gain.

## 17. Notifications & Sound

- **Toasts** — delivery modes: off / in-app / terminal escapes (good over
  SSH) / OS-native. In-app toasts are clickable to jump to the target pane.
  Configurable position and delay (notify only if the state persists);
  suppressed for the active tab.
- **Clipboard feedback** — separate popup confirming copies.
- **Notification API** — `cdock notification show <title> [--body --position
  --sound]`; sanitized and rate-limited socket method.
- **Sound** — distinct done vs needs-input sounds, on by default, client-side.
  Custom audio file paths, per-agent overrides/muting, global disable via
  config or `CDOCK_DISABLE_SOUND`. Uses system audio players; no bundled
  audio stack.

## 18. Terminal & Shell Behavior

- **Default shell** — configurable executable for new panes (falls back to
  `$SHELL`, then a platform default).
- **Shell startup mode** — auto / login / non-login (auto uses login shells on
  macOS for PATH setup).
- **New-pane cwd policy** — follow focused pane / home / client cwd / fixed
  path.
- **Scrollback** — configurable byte limit; scrollbar for primary-screen
  output.
- **Rich terminal compatibility** — OSC 8 hyperlinks, OSC 10/11/12 color
  queries, OSC 4/52 clipboard, XTGETTCAP, kitty keyboard protocol, bracketed
  multiline paste, undercurl, grapheme/emoji rendering, 256-color TERM with
  truecolor.
- **Nested-launch guard** — refuses to run inside one of its own panes unless
  explicitly allowed.

## 19. Configuration System

- **Config file** — TOML at `~/.config/comind-dock/config.toml` (platform
  equivalents elsewhere); zero-config startup works. `cdock --default-config`
  prints annotated defaults; invalid values fall back with startup warnings.
- **In-app settings screen** — a TUI settings dialog (opened on first run and
  via a prefix binding) for notifications, themes, experiments, and the
  integrations tab; changes apply live without editing the file by hand.
- **Live reload** — `cdock server reload-config` applies UI settings without
  restarting panes; produces a diagnostics report.
- **Env overrides** — config path, session name, socket path, log filter,
  sound disable, agent hint, remote binary path.
- **Logs** — capped rotating per-mode logs (combined / client / server) with
  env-controlled filtering.
- **Mobile layout** — single-column narrow-terminal layout below a
  configurable width threshold; agents-first switcher rendering worktrees as
  a tree.

## 20. Platform Support

- **Linux / macOS** — stable, full feature set, x86_64 and aarch64.
- **Windows (beta)** — ConPTY-based: persistent sessions, native panes,
  cmd/PowerShell, agent process-tree detection, integrations, plugins, pane
  screen history. Not supported: direct terminal attach, native remote
  attach, live handoff. Paste via `ctrl+shift+v`; drawn cursor by default;
  versioned install junction for updates.

## 21. Agent Profiles, Skills, Orchestration

Skills and agent roles are explicit, declarative configuration — a visible
block you edit and version, not hidden per-agent wiring.

- **Skill catalog** — a named registry of skills in config: each skill has a
  source (local path or git ref), a version, and a description. `cdock skill
  list|add|remove` manage the catalog; skills are inert until assigned to a
  profile.
- **Agent profiles** — one directory per profile
  (`~/.config/comind-dock/agents/<name>/`):
  - `profile.toml` — which agent CLI to run (argv), model, env, default cwd
    policy, the list of assigned skills, and flags (`orchestrator`,
    `memory`).
  - `agent.md` — the role definition: who this agent is, what it does, its
    constraints (e.g. "YouTube scriptwriter", "release auditor").
  - `memory.md` (opt-in) — persistent per-role memory. The profile instructs
    the agent to append lessons after each session; the file survives
    restarts and rides along into every future launch of that profile.
- **Materialization** — launching a profile stages its skills and `agent.md`
  into the locations the target agent CLI actually reads (per-agent adapters,
  the same machinery integrations use), sets `CDOCK_AGENT_PROFILE_DIR`, and
  spawns the pane. The same profile can therefore back different agent CLIs.
- **Launch by profile** — `cdock agent start --profile <name>` anywhere a
  plain agent start works (CLI, sidebar new-pane picker, right-click menu).
  Profile-launched panes show the profile name as their agent label.
- **Orchestrator profiles** — a profile flagged `orchestrator` additionally
  receives the runtime skill file plus roster access: it can enumerate
  profiles, spawn sub-agents by profile into new panes/tabs, and manage them
  with the existing automation primitives (read output, send input, wait on
  status). One conductor, many specialists, each with its own skill set and
  role file — all visible as ordinary panes.
- **Profile & skill editor (UI)** — a built-in editor dialog, opened with a
  sidebar button (and a prefix binding / settings entry), following the same
  modal language as Settings: create and edit profiles (role text, agent CLI,
  model, env), assign skills from the catalog with checkboxes, manage the
  skill catalog itself, and launch a profile straight from the editor. The
  editor reads and writes the same plain files — UI and files are two views
  of one config, so dotfiles workflows keep working.
- **Workspaces as categories** — no separate tag system: the workspace is the
  category (as in "YouTube" → scriptwriter, analyst, motion designer).
  Profiles can be associated with workspaces (`workspaces = [..]` in
  `profile.toml` or from the editor); a workspace's sidebar section and
  new-pane picker surface its associated profiles first for one-click
  launches, unassociated profiles stay available globally.
- **Profile CLI** — `cdock profile list|show|new <name>`; profiles are plain
  files, editable directly or through the UI editor.

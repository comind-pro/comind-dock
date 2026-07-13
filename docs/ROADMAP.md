# Roadmap

**Status: the core of all six phases shipped** (v0.4.x). This file is kept
as the historical plan; per-phase items that did NOT ship are struck from
the shipped lists and collected under "Open items" at the bottom.

Six phases. Each phase ships something usable; later phases never require
rewriting earlier ones because the architecture rules (pure state, pure
render, platform isolation, server-neutral naming) apply from day one.

## ✅ Phase 1 — Core multiplexer (MVP)

Single-process TUI, no server split yet (but state/runtime separation from
the start so the split is a transport change, not a rewrite).

- PTY spawning + async per-PTY actors.
- Embedded VT emulation: parsing, scrollback, modes, damage tracking.
- Workspace → tab → pane model with binary split tree (split, focus, resize,
  zoom, swap, close).
- Pure render pipeline (`compute_view` / `render`), sidebar, tab bar.
- Prefix-key input model (terminal / prefix modes), default bindings,
  `prefix+?` help.
- Mouse basics: click focus, drag borders, drag-select copy, scroll.
- TOML config: theme, terminal, keys; `--default-config`.
- Logging, nested-launch guard.

**Done when:** daily-drivable as a mouse-first terminal multiplexer.

## ✅ Phase 2 — Server/client split & persistence

- Headless server owning PTYs/emulators; thin client with hello/welcome
  handshake, protocol version, semantic-frame encoding + client-side diff.
- Auto-start-then-attach launch flow; detach/reattach; multi-client attach.
- Named sessions with per-session sockets and state dirs.
- Session snapshot persistence (debounced) + restore on server start.
- `--no-session` escape hatch.
- Folder-scoped attach (`-f`): per-client view filtered to workspaces under a
  folder, with widen-scope toggle.
- Pre-diffed ANSI encoding for bandwidth-constrained links.

**Done when:** close the terminal, reattach, everything is still there.

## ✅ Phase 3 — Agent awareness

- Detection engine: bottom-buffer snapshots, TOML manifests, region/priority/
  gate rule semantics, explain trace.
- Status model (blocked/working/done/idle/unknown) + rollups + agent panel.
- Manifest precedence (bundled < override), hot reload. (No remote manifest
  feed — new manifests ship with each release.)
- Notifications: toasts (app/terminal/system), delay/persistence logic,
  clickable jump-to-pane.
- Sounds: done vs needs-input, per-agent overrides.
- Initial manifests for the most popular agent CLIs.

**Done when:** the sidebar reliably tells you which agent needs you.

## ✅ Phase 4 — Automation surface

- JSON socket API: request/response, event subscriptions, one-shot waits,
  machine-readable command reference (`cdock api reference`), bootstrap
  snapshot. (Formal JSON Schema: open item.)
- CLI wrappers: pane/tab/workspace/agent create-focus-close, read/send/
  run, `wait output`, `wait agent-status`. (`status`, completions, and the
  full CRUD verb set: open items.)
- Pane env injection (`CDOCK_ENV`, ids, socket/bin paths).
- Agent skill file so agents can drive the runtime from inside panes.
- Direct pane attach and observe (`cdock pane attach|observe`).

**Done when:** an agent inside a pane can spawn a sibling, run tests in it,
and wait for the result — no human in the loop.

## ✅ Phase 5 — Ecosystem

- Git worktrees: grouped child workspaces, create/open/remove, safe deletion.
- Integrations: per-agent hook installers (lifecycle authority + session
  identity roles), report-agent API. (Uninstall/status reporting: open
  items.)
- Native agent session resume after restart.
- Agent profiles & skills: explicit skill catalog, per-profile
  `profile.toml`/`agent.md`/`memory.md`, per-agent-CLI materialization
  adapters, launch by profile, orchestrator profiles with roster access,
  workspace-as-category association. (Profile & skill editor UI: open item.)
- Plugins: manifest, out-of-process actions, event hooks, link handlers,
  managed panes, install from GitHub shorthand, local link for development.
  (Marketplace index: open item.)

**Done when:** third parties can extend the runtime without forking it.

## ✅ Phase 6 — Distribution & reach

- Update system: stable/preview channels (GitHub Releases feed), mandatory
  checksum verification, release notes on startup.
- Live handoff: upgrade or replace a running server without killing panes.
- SSH-in workflows: `cdock session attach ssh:<host>`, `workspace create
  --ssh`. (Native remote attach and remote bridges: open items.)
- Install scripts, Nix flake, mise; four release targets
  (linux/macos × x86_64/aarch64). (Homebrew tap: open item.)
- Copy-mode search, scrollback editor, mobile layout polish.

**Done when:** `curl | sh` on a fresh machine gives the full experience, and
`cdock update` keeps it fresh.

## Open items (deliberate deferrals)

- Windows/ConPTY — no Windows machine to validate against.
- kitty graphics protocol — ratatui's cell renderer cannot show raster.
- IME composition — crossterm exposes no IME events.
- Homebrew tap — the formula lives in `packaging/homebrew/`; it needs a
  `comind-pro/homebrew-tap` repo before `brew install` works.
- Remote attach (`cdock --remote <host>`) and per-workspace remote bridges
  (`remote mount`, host badges, reconnect) — designed, not started.
- Formal JSON Schema for the socket API (`api reference` is the
  machine-readable catalog today).
- CLI gaps: `cdock status`, shell completions, pane
  move/get/layout/process-info, workspace/tab rename-list-get,
  `pane send-keys`, layout export/apply, `integration uninstall|status`,
  plugin uninstall/enable/disable + marketplace, notification API,
  `cdock channel`, profile & skill editor UI.

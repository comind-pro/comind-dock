# comind-dock

A terminal-native runtime and multiplexer for AI coding agents.

comind-dock lets you run many coding agents side by side in one terminal:
each agent lives in its own persistent pane, the runtime detects whether an
agent is working, blocked, or done, and surfaces the ones that need your
attention. Detach and reattach at will — agents keep running on a background
server. Drive everything with the keyboard, the mouse, or a scriptable
CLI/JSON API that agents themselves can use to coordinate with each other.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/comind-pro/comind-dock/master/install.sh | sh
```

Installs the latest release binary to `~/.local/bin/cdock` (override with
`CDOCK_INSTALL_DIR`). Update later from inside the dock — the sidebar menu
shows "update ready" when a new release ships — or with `cdock update
--handoff`, which swaps the running server in place without killing a
single pane.

### Nix

```sh
nix run github:comind-pro/comind-dock
```

## Status

**All roadmap phases shipped** (current release: v0.4.x). Highlights:

- Background server owns the panes; thin clients attach/detach at will,
  named sessions (`cdock --session`), remote attach over ssh.
- Agent detection engine (TOML manifests + `cdock agent explain` rule
  traces) with sounds/toasts when an agent blocks or finishes; hooks can
  report authoritative states (`cdock pane report-agent`).
- Exact session continuation: each claude pane resumes ITS conversation
  (SessionStart hook, multiple `~/.claude*` profiles), codex/opencode
  bind by session files; screen history replays above the fresh prompt.
- Live handoff: `cdock update --handoff` swaps the running server binary
  in place — no pane dies. Self-update from GitHub Releases with
  stable/preview channels.
- Agent profiles (roles): global and workspace-scoped, created from the
  UI or CLI, attachable to a RUNNING agent (`cdock agent behavior`);
  skill catalog; orchestrator profiles that spawn specialist subagents.
- Full automation surface: every UI action is also a CLI command and a
  JSON socket API (`cdock api schema`), with event subscriptions,
  `wait` primitives, and two-way `cdock pane attach`.
- Plugins: linked or `gh:owner/repo`-installed, with actions, managed
  panes, and agent-status hooks. Git worktrees as child spaces.

Deliberate deferrals: Windows/ConPTY, kitty graphics, IME composition
(details in [docs/ROADMAP.md](docs/ROADMAP.md)).

Build and run from source:

```sh
cargo build --release
./target/release/cdock
```

## Core ideas

- **Server-owned sessions.** A background server owns every terminal and
  process. Clients are thin and disposable: close your terminal, reattach
  later, nothing dies.
- **Agents are first-class.** The runtime classifies each agent pane as
  working / blocked / done / idle and rolls that state up to tabs and
  workspaces, so a glance at the sidebar tells you where you're needed.
- **Automation-friendly by design.** The entire UI surface is also a CLI and
  a JSON socket API. Agents running inside panes can spawn siblings, read
  their output, and wait on their state.
- **Mouse-first TUI.** Click, drag, right-click menus, scroll — everything
  the keyboard can do, the mouse can too.

## Documentation

| Document | Contents |
|---|---|
| [docs/FEATURES.md](docs/FEATURES.md) | Complete feature catalog, grouped by category |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Target architecture: server/client split, state model, detection engine |
| [docs/CLI.md](docs/CLI.md) | CLI command specification |
| [docs/CONFIGURATION.md](docs/CONFIGURATION.md) | Config file format, settings, environment variables |
| [docs/ROADMAP.md](docs/ROADMAP.md) | Implementation phases |

## Contributing

Open source under MIT, standard PR flow. `cargo clippy --all-targets`
clean and `cargo test` green before every PR; new behavior needs a test.
For local development use the isolated dev namespace (`ln -sf cdock
target/debug/cdock-dev`) so your live session stays untouched.

## License

[MIT](LICENSE)

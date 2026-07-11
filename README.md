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

### Homebrew

```sh
brew install comind-pro/tap/cdock
```

### Nix

```sh
nix run github:comind-pro/comind-dock
```

## Status

**Phase 1 (core multiplexer MVP) implemented.** `cdock` is a working
single-process terminal multiplexer: PTY panes with full VT emulation,
workspace → tab → pane split tree, prefix-key input model with a
configurable keymap, mouse support (click focus, border drag, selection
copy, scroll), and TOML configuration (`cdock --default-config`).
Server/client split and persistence land in Phase 2; the full plan is in
[docs/ROADMAP.md](docs/ROADMAP.md).

Build and run:

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

The project is open source under MIT. During the specification phase,
contributions are welcome as issues and discussion on the feature and
architecture documents. Once Phase 1 starts, standard PR flow applies.

## License

[MIT](LICENSE)

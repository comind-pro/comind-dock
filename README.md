# comind-dock

Run a dozen AI coding agents side by side, in one terminal, without losing
track of which one needs you.

`cdock` is a terminal multiplexer built for agents rather than for shells:
each agent lives in a persistent pane, the dock knows whether it is
**working**, **blocked** (waiting on you) or **done**, and it says so — in
the sidebar, with a sound, with a toast you can click. Close the terminal
and the agents keep running; reopen it and each one resumes *its own*
conversation.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/comind-pro/comind-dock/master/install.sh | sh
```

The binary lands in `~/.local/bin/cdock`. Nix: `nix run github:comind-pro/comind-dock`.

## First five minutes

```sh
cdock integration install claude   # let claude report its own status (once)
cdock                              # you are in a normal shell, in a pane
```

Type `claude` in the pane and work as usual. Now:

- **The sidebar** (left) lists your *spaces* (one per folder/repo) and every
  agent in them, with its status, its profile, and which space it lives in.
- **Right-click anything** — a pane, a tab, an agent row, a space — for its
  menu: rename, split, close, new agent, attach a role.
- **Detach** with `ctrl+b q`. The agents keep running. `cdock` again to come
  back. Everything you had is still there, including the screen history.
- **Split** with `ctrl+b v` (right) or `ctrl+b -` (below); new tab `ctrl+b c`;
  switch tabs with `ctrl+b 1..9`; `ctrl+b ?` lists every binding.

When an agent blocks on a permission prompt while you're in another tab,
you get a sound and a clickable toast. That is the whole point of the thing.

## What makes it different from tmux

- **It knows what agents are doing.** Status comes from claude's own hooks
  (screen-reading is the fallback), so "blocked" means blocked, not "output
  stopped."
- **Sessions resume exactly.** Not "a shell in the same folder" — *that*
  conversation, in the right claude profile, in the right directory.
- **Agents can drive it.** Every menu action is also a CLI command and a JSON
  socket API (`cdock api schema`). An agent in a pane can spawn helpers, read
  their screens, wait on their status — and it ships with a skill telling it how.
- **Roles, not just commands.** Write an agent role once (`cdock profile new
  reviewer`), attach it to a running session, or let an agent author roles for
  its own workspace and spawn subagents with them.
- **Updates without losing work.** `cdock update --handoff` replaces the
  running binary in place. Not one pane dies.

## Cost of running it

Ten agents streaming output continuously, measured on the release build:
**47 MB, ~5% of one core** with the UI attached; **40 MB, ~1%** detached
(the agents keep running, nothing is rendered). Idle panes cost nothing.

## Documentation

| Document | Contents |
|---|---|
| [docs/CLI.md](docs/CLI.md) | Every command, and the JSON API behind it |
| [docs/CONFIGURATION.md](docs/CONFIGURATION.md) | Config file, profiles, skills, env vars |
| [docs/FEATURES.md](docs/FEATURES.md) | The full feature catalog |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Server/client split, state model, detection |
| [docs/ROADMAP.md](docs/ROADMAP.md) | What shipped, and what is deliberately not built |

Not built (on purpose): Windows, inline images, IME composition. Details in
the roadmap.

## Contributing

MIT, standard PR flow. `cargo clippy --all-targets` clean and `cargo test`
green before every PR; new behavior needs a test. Develop against the
isolated dev namespace (`ln -sf cdock target/debug/cdock-dev`) so your own
live session stays untouched.

## License

[MIT](LICENSE)

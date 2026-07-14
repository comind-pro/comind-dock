# Architecture

> Written as the target design; as of v0.4.x the core is the implemented
> architecture. Not yet built: the remote bridge layer (§4a — today remote
> means `cdock session attach ssh:host`, a remote cdock over `ssh -t`), the
> remote manifest feed (§4 mentions it as a precedence tier), and a formal
> JSON Schema (the API ships a machine-readable example catalog instead;
> requests are `{"cmd": …}` lines, not `{id, method, params}`).
>
> **Per-client views.** Panes and their processes belong to the session, but
> the *view* belongs to the client: each attached terminal keeps its own
> folder scope (`-f`), its own active workspace, and its own size, and the
> server renders one frame per client (`server::enter`/`leave` swap the
> client's view state into the runtime around every render and input event).
> Two clients that open the SAME workspace share the window — same panes,
> mirrored — and the shared pty takes the smallest viewer's size so neither
> sees it cropped. Automation focus (`cdock workspace focus`) is pushed to
> every client, since no terminal owns the API.

comind-dock ships as a single binary that wears different hats depending on
argv: persistent headless server, thin TUI client, remote SSH bridge, CLI
subcommand, or a monolithic single-process fallback for debugging.

## 1. Server / client split

The **server** is a background daemon that owns all durable state: workspaces,
tabs, panes, PTYs, terminal emulators, agent detection, persistence. The
**client** is thin and disposable: it connects, reports its size and
capabilities, receives rendered frames, blits them to the host terminal, and
forwards input. Multiple clients may attach to one session.

Default launch flow (`cdock` with no args):

1. Probe the session's client socket. A live server → attach.
2. No server → spawn a detached daemon seeded with the launch cwd, wait for
   its socket to appear (bounded, ~15 s).
3. Attach as a thin client.

`--no-session` bypasses the split and runs everything in one process (used
mainly for tests and debugging).

**Named sessions** give each session its own socket paths and state directory;
config is shared.

## 2. Two socket surfaces

Per session, the server exposes two independent sockets:

- **Public JSON API socket** — newline-delimited JSON requests
  (`{"cmd": "...", …}` per line, one reply line each), streaming event
  subscriptions, and a machine-readable command catalog (`cdock api
  reference`). This is the stable surface: the entire CLI wraps it, plugins
  and agents script against it, and third-party integrations report agent
  state through it.
- **Private binary client protocol** — a high-throughput frame/input channel
  used only by the TUI client. Length-prefixed frames (u32 prefix, hard size
  caps), compact binary serialization, explicit `PROTOCOL_VERSION` negotiated
  in a hello/welcome handshake.

**Boundary rule:** anything that is a shared runtime/session fact must be
reachable through the JSON API and its events. The private client socket
carries only presentation traffic. Server-side names stay neutral
(pane/terminal/agent), never UI-surface names (sidebar/row/card).

### Client protocol messages (shape, not wire format)

- Client → server: hello (size, cell pixel size, protocol version, requested
  render encoding, keybinding mode, launch mode app/terminal-attach), raw
  input bytes, structured input events (for platforms without raw stdin),
  resize, paste, clipboard image, detach, direct attach/observe/control
  requests, attach-scroll.
- Server → client: welcome (version, chosen encoding, optional error), frame,
  graphics payload, shutdown notice, notification (sound/toast), clipboard
  forward, window title, config-reload notices, mouse-capture and
  input-source directives.

### Render encodings

Negotiated per connection:

- **Semantic cell grid** — a full frame as packed cells (fg/bg, modifiers,
  hyperlink indices, skip flags) plus cursor state and graphics. The client
  diffs against its previous frame before writing to the host terminal.
  Default for local attach.
- **Pre-diffed ANSI** — the server itself diffs and sends ready-to-write ANSI
  byte streams. Cheaper on bandwidth; used for remote links.

## 3. State discipline

Load-bearing rules, enforced by tests:

- **Pure state vs runtime.** The application state object is plain data,
  constructible in unit tests without PTYs or async. The runtime wrapper adds
  event channels, the live terminal registry, timers (render throttle ~16 ms,
  git refresh, update checks, debounced session saves), and the async event
  loop. Pane state (pure) is likewise separate from pane runtime (live PTY).
- **Pure render.** A two-phase pipeline per frame: `compute_view` does all
  geometry and mutation (pane rects, PTY resizes, sidebar clamping);
  `render` takes an immutable state reference and only draws. Never mutate
  during render. The server runs this per attached client at that client's
  size and encoding.
- **Hierarchy.** Workspace → tab → pane split tree. The split tree is a
  binary layout tree keyed by pane id, holding directions and ratios. Ids are
  short public codes. Per-pane agent state rolls up through tab and workspace
  into attention summaries consumed by the sidebar and notifications.
- **Invariant tests.** Identity/state refactors are release-risk; the state
  types expose test-only invariant assertions and adversarial-state
  constructors.

## 4. Agent detection engine

A screen-snapshot pattern matcher, decoupled by design: it reads a snapshot,
never touches live parser state, and classifies from the **bottom buffer**,
not the user-scrollable viewport (users can scroll the viewport away from the
live prompt).

- **Input:** the pane's bottom-of-buffer text plus OSC-derived strings (title,
  progress).
- **Manifests:** one TOML file per agent — id, version, minimum engine
  version, aliases, and an ordered rule list.
- **Rules:** each targets a region ("bottom" non-empty lines or the OSC
  title), carries a priority and a target state, and matches via explicit
  AND/OR/NOT gates (`all_of`/`any_of`/`none_of`, case-insensitive
  substrings; regex/line-regex is a design option, not built).
  Highest-priority match wins.
- **Output:** state + skip flag + evidence flags, plus a full explain trace
  (matched rule, evaluated rules, evidence, manifest source and versions)
  surfaced by the explain CLI.
- **Source precedence:** bundled (compiled in, source of truth) < local
  override. Hot reload without restart (`cdock server reload-manifests`).
  New bundled manifests ship with each release; a fetched remote tier is a
  design option, not built.
- **Authority chain:** integration hook authority (authoritative while live)
  > screen detection (fallback/recovery) > unknown; process exit clears hook
  authority and triggers recompute. PTY output activity is the routine
  "working" signal.

## 4a. Remote bridges (mixed local/remote) — design, not yet built

Two remote modes share the same SSH transport:

- **Full remote attach** (`--remote`): the local process is a pure thin
  client; everything lives on the remote server. Simple, unchanged.
- **Remote bridge** (`cdock remote mount`, `workspace create --host`): the
  **local server** owns an SSH bridge per remote host. The bridge speaks the
  remote server's JSON API and frame streams and projects the remote
  workspaces into the local session's state as remote-backed workspaces
  (tagged with an origin host). Clients see one unified snapshot — sidebar,
  agent panel, rollups, and notifications span local and remote alike.

Ownership rule: the remote server remains the authority for its own panes,
processes, agents, and persistence. The bridge is a proxy, never a second
owner — a lost connection marks the projected workspaces disconnected (and
reconnects with keepalive) but affects nothing running remotely. Input and
frames for remote panes pass through the local server to the bridge;
detection runs on the remote side where the screen lives. Because the
projection is server-side, every client and API consumer sees the same mixed
topology, and folder scoping / navigation work uniformly across origins.

## 5. Terminal emulation

- **Embedded VT engine.** Options, all license-compatible with MIT:
  vendor a mature MIT-licensed VT core, or use a Rust terminal-emulation
  crate (e.g. the emulation layers extracted from established Rust
  terminals). Requirements: full VT/xterm parsing, scrollback, grapheme/emoji
  width, mode tracking, damage/dirty tracking for cheap diffs.
- **PTY layer.** A cross-platform PTY backend (forkpty on Unix, ConPTY on
  Windows) with one async read/write actor per PTY feeding bytes into the
  emulator.
- **Per-pane features.** Cursor shape/visibility, OSC handling (title,
  progress, cwd, OSC 52 clipboard), kitty keyboard protocol state, XTGETTCAP
  responses, mode tracking, appearance queries.
- **Kitty graphics.** Image payloads ride the frame channel with a raised
  size cap; the client blits to graphics-capable host terminals. Opt-in.
- **Scrollback** capped by a configurable byte limit (default ~10 MB).

## 6. Persistence

Stored in the session's state directory:

- **Session snapshot** (JSON): workspaces, tabs, layout trees, cwds, agent
  metadata. Typed snapshot structs; saves debounced (~5 s) and run off the
  main loop.
- **Screen history** (optional, opt-in): recent pane contents for replay
  after restart.
- **Plugin registry** (JSON).

Restore rebuilds workspaces/tabs/panes and cwds on server start. **Agent
resume:** panes whose integrations reported a native session reference (id or
path) get a resume plan (agent argv + dedupe key) and relaunch into their own
conversation. A separate handoff-restore path covers live handoff.

## 7. Platform isolation

One boundary module holds shared traits and types (foreground process info,
signals, clipboard types) plus a capability struct (live handoff, remote
attach, direct terminal attach — Unix-only, compiled out elsewhere).
OS-specific bodies live in per-OS files, compile-gated so no foreign-OS code
builds into a target. Core modules contain no OS conditionals. Targets:
Linux and macOS (full), Windows (beta: ConPTY, named-pipe sockets, structured
input events, PowerShell hook variants), plus a stub fallback.

## 8. Update system

- Two channels built from one main branch: **stable** (default) and
  **preview** (opt-in prereleases). Each reads a static JSON feed:
  `{version, protocol, notes, assets: {target → url}, releases: {…}}`.
- Background version check (~30 min) only surfaces availability and notes.
  Explicit `cdock update` downloads the matching asset, verifies a SHA-256
  checksum, and swaps the binary. Package-manager installs are detected and
  given the right upgrade command instead of self-replacing.
- Fetching shells out to `curl` — deliberately no HTTP client dependency.
- `--handoff` upgrades a running server without killing agent processes.
- Detection-manifest updates are a separate background feed (see §4).

## 9. Dependency posture

Lean by policy. The expected core set for a Rust implementation:

| Role | Candidate |
|---|---|
| TUI buffers/widgets | ratatui |
| Host terminal control & input | crossterm |
| Async runtime | tokio |
| PTY spawning | portable-pty |
| Local sockets (UDS) | tokio UnixListener/UnixStream |
| VT emulation | alacritty_terminal |
| Serialization | serde + serde_json (API/persistence), a compact binary codec (wire), toml (config/manifests) |
| Manifest matching | case-insensitive substrings (no regex dependency) |
| CLI parsing | clap (completions: planned) |
| Checksums | system `shasum`/`sha256sum` (no crypto dependency) |
| Width measurement | unicode-width |
| Logging | tracing |

Conventions: no `unwrap()` in production paths, structured logging over
prints, `#[allow]` only with a justification comment, new dependencies only
when existing ones can't cover the need.

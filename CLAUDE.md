# comind-dock — agent rules

## Testing server features: cdock-dev ONLY

NEVER run test servers, scoped attaches (`-f`), workspace/tab mutations, or
handoffs against the default session. The user's live session autosaves every
5 seconds — one stray attach permanently overwrites their real workspaces
(this has happened; recovery cost an evening).

Local development uses `cdock-dev` — a symlink to the same binary that flips
it into a fully isolated namespace (own state dir, sockets, snapshot;
nested-launch guard off). Create it once per target dir:

```bash
cargo build && ln -sf cdock target/debug/cdock-dev
./target/debug/cdock-dev --server &
./target/debug/cdock-dev pane list          # every test command: the -dev name
```

(`CDOCK_DEV=1 ./target/debug/cdock …` is the equivalent env form.)

For tests that must not see even the dev session, add a throwaway state dir:
`XDG_STATE_HOME=$(mktemp -d /tmp/cdk.XXXX)` (short path — unix sockets cap
at ~104 chars).

The production session (`cdock`, no overrides) belongs to the user. Read from
it only; never write, attach, or mutate.

Killing test servers: NEVER `pgrep -f "cdock --server" | kill` — it matches
the production server. Kill only pids you recorded when starting the dev
server (`$!`), or match the dev name exactly: `pgrep -f "cdock-dev --server"`.

## Verify before done

`cargo clippy --all-targets` clean and `cargo test` green before every
commit. New behavior gets a test or a sandboxed end-to-end check.

## Rollout

New binary into the live session: `cargo build --release && cdock server
handoff` (panes survive; the user runs this, not you).

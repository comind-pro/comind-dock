# comind-dock — agent rules

## Testing server features: SANDBOX ONLY

NEVER run test servers, scoped attaches (`-f`), workspace/tab mutations, or
handoffs against the default session. The user's live session autosaves every
5 seconds — one stray attach permanently overwrites their real workspaces
(this has happened; recovery cost an evening).

```bash
D=$(mktemp -d /tmp/cdk.XXXX)             # short path: unix sockets < 104 chars
XDG_STATE_HOME=$D ./target/debug/cdock --server &
XDG_STATE_HOME=$D ./target/debug/cdock pane list   # every test command too
kill %1 && rm -rf $D
```

The default session (no XDG_STATE_HOME override) belongs to the user. Read
from it only; never write, attach, or mutate.

## Verify before done

`cargo clippy --all-targets` clean and `cargo test` green before every
commit. New behavior gets a test or a sandboxed end-to-end check.

## Rollout

New binary into the live session: `cargo build --release && cdock server
handoff` (panes survive; the user runs this, not you).

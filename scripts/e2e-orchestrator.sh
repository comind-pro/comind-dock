#!/bin/sh
# Sandboxed end-to-end checks for the orchestration layer: task results,
# teams, waits, handoff transparency, nudges. Runs a throwaway cdock-dev
# server under a temp XDG_STATE_HOME — never touches the live session.
#
#   cargo build && ln -sf cdock target/debug/cdock-dev
#   sh scripts/e2e-orchestrator.sh
set -u
cd "$(dirname "$0")/.."
# Run from inside a cdock pane? Our own pane id must not leak into tests.
unset CDOCK_PANE_ID
BIN=./target/debug/cdock-dev
[ -x "$BIN" ] || { echo "build first: cargo build && ln -sf cdock target/debug/cdock-dev"; exit 2; }
D=$(mktemp -d /tmp/cdk.XXXX)
start() { XDG_STATE_HOME=$D CDOCK_STALL_SECS=4 $BIN --server >>"$D/server.log" 2>&1 & SRV=$!; sleep 1; }
C() { XDG_STATE_HOME=$D CDOCK_CONFIG_PATH=$D/cfg/config.toml $BIN "$@"; }
pid_of() { sed -n 's/.*"pane":\([0-9]*\).*/\1/p'; }
flat() { C pane read "$1" --lines 30 | python3 -c "import sys,json;print(''.join(json.load(sys.stdin)['text'].split()))"; }
pass=0; fail=0
ok() { pass=$((pass+1)); echo "PASS: $1"; }
bad() { fail=$((fail+1)); echo "FAIL: $1"; }
trap 'kill $SRV 2>/dev/null; rm -rf "$D"' EXIT
start

# --- results -----------------------------------------------------------
A=$(C agent start '"$CDOCK_BIN" task done "hello from A" && sleep 300' | pid_of)
B=$(C agent start '"$CDOCK_BIN" task done "hello from B" && sleep 300' | pid_of)
C wait task-result "$A" --timeout 10000 | grep -q '"result":"hello from A"' && ok "wait task-result" || bad "wait task-result"
C task result "$A" | grep -q '"error":"no result' && ok "consume-on-read" || bad "consume-on-read"
S=$(C pane split 1 --direction right | pid_of)
C wait agent-status "$S" --status idle --timeout 30000 >/dev/null   # shell up (slow under load)
sleep 2
C pane run "$S" '"$CDOCK_BIN" task done "second" --pane '"$A" >/dev/null
C wait task-result "$A" --timeout 30000 | grep -q '"result":"second"' && ok "task result fetch" || bad "task result fetch"

# --- waits -------------------------------------------------------------
sleep 3
C wait agent-status "$B" --status idle --transition --timeout 3000 | grep -q '"error":"timeout"' \
  && ok "transition blocks on already-idle" || bad "transition"
C wait agent-status "$B" --status idle --timeout 3000 | grep -q '"ok":true' && ok "plain wait instant" || bad "plain wait"

# --- teams + task lifecycle -------------------------------------------
C team set "$A" "$B" >/dev/null
R=$(C team list --orchestrator "$B")
echo "$R" | grep -q "\"worker\":$A" && echo "$R" | grep -q '"cwd"' && echo "$R" | grep -q '"workspace_id"' \
  && ok "team list context" || bad "team list: $R"
C task done "ts" --pane "$A" >/dev/null
C team list --all | grep -q '"task_state":"reported"' && ok "task_state reported" || bad "reported"
C task result "$A" >/dev/null
C team list --all | grep -q '"task_state":"collected"' && ok "task_state collected" || bad "collected"
C pane run "$A" --notify ": fyi" >/dev/null
C team list --all | grep -q '"task_state":"collected"' && ok "run --notify keeps collected" || bad "notify re-armed"
C pane run "$A" ":" >/dev/null
C team list --all | grep -q '"task_state":"assigned"' && ok "pane run assigns" || bad "assigned"
C team park "$A" >/dev/null
C team list --all | grep -q '"task_state":"collected"' && ok "team park" || bad "park"
C team note "$A" "waiting on CI" >/dev/null
C team list --all | grep -q '"note":"waiting on CI"' && ok "team note" || bad "note"
printf ': from file `echo SHOULD_NOT_RUN`\n' > "$D/prompt.txt"
C pane run "$S" --file "$D/prompt.txt" >/dev/null; sleep 2
C pane read "$S" --plain --lines 20 | grep -q "^SHOULD_NOT_RUN" && bad "--file executed backticks" || ok "run --file literal"
C pane read "$S" --plain --lines 3 | grep -q '^{' && bad "plain read is JSON" || ok "pane read --plain"
C team set "$A" none >/dev/null
C team list --all | grep -q '"teams":\[\]' && ok "team clear" || bad "team clear"

# --- handoff transparency ---------------------------------------------
C task done "survives handoff" --pane "$A" >/dev/null
C server handoff >/dev/null; sleep 5
C task result "$A" | grep -q '"result":"survives handoff"' && ok "result survives handoff" || bad "handoff result"
C wait task-result "$A" --timeout 30000 >"$D/w.out" 2>&1 & W=$!
sleep 1; C server handoff >/dev/null; sleep 5
C task done "after handoff" --pane "$A" >/dev/null; wait $W
grep -q '"result":"after handoff"' "$D/w.out" && ok "wait rides through handoff" || bad "handoff wait"

# --- orchestrator nudges -----------------------------------------------
mkdir -p "$D/cfg/agents/testorch"
printf 'command = "sh"\norchestrator = true\n' > "$D/cfg/agents/testorch/profile.toml"
ORCH=$(C agent start --profile testorch | pid_of)
WK=$(C agent start 'printf "\033]0;claude\007"; sleep 300' | pid_of)
sleep 2; C team set "$WK" "$ORCH" >/dev/null; C team mode "$ORCH" auto >/dev/null
C pane report-agent "$WK" blocked >/dev/null; sleep 2
flat "$ORCH" | grep -q "isblockedawaitinginput" && ok "blocked nudges" || bad "blocked nudge"
C task done "real" --pane "$WK" >/dev/null; sleep 2
flat "$ORCH" | grep -q "reportedaresult" && ok "task done nudges" || bad "result nudge"
C pane run "$ORCH" "hi" >/dev/null   # from = unset in this shell → no prefix expected
C task result "$WK" >/dev/null; C pane run "$WK" "next" >/dev/null; C pane report-agent "$WK" clear >/dev/null
sleep 10
N=$(flat "$ORCH" | grep -o "withoutreportingaresult" | wc -l | tr -d ' ')
sleep 6
N2=$(flat "$ORCH" | grep -o "withoutreportingaresult" | wc -l | tr -d ' ')
[ "$N" -ge 1 ] && [ "$N" = "$N2" ] && ok "stall warns once" || bad "stall: $N -> $N2"
env CDOCK_PANE_ID="%$WK" XDG_STATE_HOME="$D" $BIN pane run "$ORCH" "from worker" >/dev/null; sleep 2
flat "$ORCH" | grep -q "\[cdock:%${WK}→%${ORCH}\]fromworker" && ok "worker message tagged" || bad "no [cdock:from] tag"
C pane close "$WK" >/dev/null; sleep 2
flat "$ORCH" | grep -q "closedandleftyourteam" && bad "pane close notified" || ok "pane close is quiet"

# --- queues, locks, provider limits -------------------------------------
Q=$(C agent start 'sleep 300' | pid_of)
C task done "one" --pane "$Q" >/dev/null; C task done "two" --pane "$Q" >/dev/null
R=$(C task result "$Q"); echo "$R" | grep -q '"result":"one"' && echo "$R" | grep -q '"pending":1' \
  && ok "results FIFO (two reports kept)" || bad "fifo: $R"
C task result "$Q" | grep -q '"result":"two"' && ok "second result kept" || bad "second result lost"
env CDOCK_PANE_ID="%$Q" XDG_STATE_HOME="$D" $BIN lock acquire main | grep -q '"ok":true' && ok "lock acquire" || bad "lock acquire"
env CDOCK_PANE_ID="%$ORCH" XDG_STATE_HOME="$D" $BIN lock acquire main | grep -q "\"held_by\":$Q" && ok "lock contention reports holder" || bad "lock contention"
C pane close "$Q" >/dev/null; sleep 2
env CDOCK_PANE_ID="%$ORCH" XDG_STATE_HOME="$D" $BIN lock acquire main | grep -q '"ok":true' && ok "lock auto-released on holder exit" || bad "lock not released"
LW=$(C agent start "printf 'You have hit your usage limit. Try again later.\\n'; sleep 300" --team "$ORCH" | pid_of); sleep 3
flat "$ORCH" | grep -q "isstoppedbyitsprovider" && ok "usage limit nudges orchestrator" || bad "limit nudge"
C team list --orchestrator "$ORCH" | grep -q '"limited":"You have hit your usage limit' && ok "team list limited" || bad "limited field"
sleep 2
C pane run "$ORCH" "plain-message" >/dev/null; sleep 4
flat "$ORCH" | grep -q "plain-message" && ok "orchestrator run delivers" || bad "orch run not delivered"
# Back-to-back messages must arrive as SEPARATE submits, not one glued input.
C pane run "$ORCH" "burst-one" >/dev/null; C pane run "$ORCH" "burst-two" >/dev/null; sleep 6
flat "$ORCH" | grep -q "burst-oneburst-two" && bad "burst messages glued" || ok "burst messages separate"
flat "$ORCH" | grep -q "burst-two" && ok "queued burst delivered" || bad "burst-two never delivered"
# An orchestrator must name the target space; a worker defaults to its own.
R=$(env CDOCK_PANE_ID="%$ORCH" XDG_STATE_HOME="$D" CDOCK_CONFIG_PATH="$D/cfg/config.toml" $BIN agent start 'sleep 300')
echo "$R" | grep -q "must pass --workspace" && ok "orchestrator spawn needs --workspace" || bad "orch spawn: $R"
R=$(env CDOCK_PANE_ID="%$S" XDG_STATE_HOME="$D" $BIN agent start 'sleep 300')
NP=$(echo "$R" | pid_of)
WS_S=$(C pane list | python3 -c "import json,sys;print([p['workspace'] for p in json.load(sys.stdin)['panes'] if p['id']==$S][0])")
WS_N=$(C pane list | python3 -c "import json,sys;print([p['workspace'] for p in json.load(sys.stdin)['panes'] if p['id']==$NP][0])")
[ "$WS_S" = "$WS_N" ] && ok "spawn lands in caller's space" || bad "spawn ws $WS_N != caller $WS_S"

# --- messaging, wake-ups, history, bulk, roles ---------------------------
P1=$(C agent start 'sleep 300' --team "$ORCH" | pid_of)
P2=$(env CDOCK_PANE_ID= XDG_STATE_HOME="$D" CDOCK_CONFIG_PATH="$D/cfg/config.toml" $BIN agent start 'sleep 300' --team "$ORCH" | pid_of)
OUT=$(C agent start 'sleep 300' | pid_of)
sleep 2
env CDOCK_PANE_ID="%$P1" XDG_STATE_HOME="$D" $BIN msg "$ORCH" "ping-from-p1" | grep -q '"ok":true' && ok "msg within team" || bad "msg within team"
sleep 3; flat "$ORCH" | grep -q "\[cdockmsg%${P1}→%${ORCH}\]ping-from-p1" && ok "msg tagged + delivered" || bad "msg not delivered"
env CDOCK_PANE_ID="%$OUT" XDG_STATE_HOME="$D" $BIN msg "$ORCH" "intruder" | grep -q "not in one team" && ok "msg outside team refused" || bad "msg scope"
C wake "tick-tock" --in 2s --pane "$ORCH" >/dev/null; sleep 5
flat "$ORCH" | grep -q "wake-up:tick-tock" && ok "wake fires" || bad "wake"
C task done "hist-1" --pane "$P1" >/dev/null; C task done "hist-2" --pane "$P1" >/dev/null
C task result "$P1" >/dev/null; C task result "$P1" >/dev/null
C task log "$P1" | grep -q "hist-1" && C task log "$P1" --last 1 | grep -q "hist-2" && ok "task log keeps history" || bad "task log"
C pane run "$P1,$P2" "bulk-task line one" | grep -c '"ok":true' | grep -q 2 && ok "bulk pane run" || bad "bulk run"
C team role "$P1" critic >/dev/null
R=$(C team list --orchestrator "$ORCH")
echo "$R" | grep -q '"role":"critic"' && echo "$R" | grep -q '"task":"bulk-task line one"' && echo "$R" | grep -q '"assigned_secs"' \
  && ok "team list role/task/assigned" || bad "team list fields"

# --- persistence -------------------------------------------------------
Z=$(C agent start 'sleep 300' --team "$ORCH" | pid_of)
C team list --orchestrator "$ORCH" | grep -q "\"worker\":$Z" && ok "agent start --team" || bad "--team"
sleep 6; kill $SRV; wait $SRV 2>/dev/null; start
C team list --all | grep -q '"orchestrator"' && ok "teams survive restart" || bad "restart"
C pane list | grep -q '"orchestrator":true' && ok "orchestrator mark survives restart" || bad "orch mark"
C team list --all | grep -q '"task_state":"assigned"' && bad "cold restore left tasks assigned" || ok "cold restore: no stale assigned"

echo "---"; echo "pass=$pass fail=$fail"
[ "$fail" -eq 0 ]

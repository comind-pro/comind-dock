#!/usr/bin/env python3
"""Sandboxed end-to-end check of the sidebar space list: sort (click the
"spaces ⇅" header), favorites (right-click → ★ favorite), filter (⌕ /
prefix+f, ✕ clears). Drives a REAL client in a pty with real mouse events
against a throwaway cdock-dev server (temp XDG_STATE_HOME) — never the live
session.

    cargo build && ln -sf cdock target/debug/cdock-dev
    python3 scripts/e2e-spaces.py
"""
import glob
import json
import os
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import time
import fcntl
import termios

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, 'target/debug/cdock-dev')
if not os.access(BIN, os.X_OK):
    sys.exit('build first: cargo build && ln -sf cdock target/debug/cdock-dev')
D = tempfile.mkdtemp(prefix='cdk.', dir='/tmp')  # short: unix sockets cap ~104 chars
ENV = {**os.environ, 'XDG_STATE_HOME': D, 'CDOCK_CONFIG_PATH': f'{D}/cfg/config.toml'}
ENV.pop('CDOCK_PANE_ID', None)  # run from inside a pane: don't leak our id
ROWS, COLS, SIDEBAR = 45, 160, 24
NAMES = ['charlie', 'alpha', 'bravo']  # creation order ≠ alphabetical
passed = failed = 0


def ok(cond, what):
    global passed, failed
    if cond:
        passed += 1
        print('PASS:', what)
    else:
        failed += 1
        print('FAIL:', what)


def C(*args):
    out = subprocess.run([BIN, *args], env=ENV, capture_output=True, text=True, timeout=30).stdout
    return json.loads(out) if out.strip().startswith('{') else {}


srv = subprocess.Popen([BIN, '--server'], env=ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
client_pid = None
try:
    time.sleep(1)
    ids = {}
    for n in NAMES:
        os.makedirs(f'{D}/{n}')
        ids[n] = C('workspace', 'create', '--cwd', f'{D}/{n}')['workspace']

    client_pid, fd = pty.fork()
    if client_pid == 0:
        os.execve(BIN, [BIN], ENV)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack('HHHH', ROWS, COLS, 0, 0))
    os.kill(client_pid, signal.SIGWINCH)

    # Just enough of a terminal to read the sidebar back: cursor moves,
    # printable text, clears. SGR and other CSI sequences are dropped.
    grid = [[' '] * COLS for _ in range(ROWS)]
    cur = [0, 0]

    def feed(data):
        nonlocal_tail = feed.tail + data
        i = 0
        while i < len(nonlocal_tail):
            ch = nonlocal_tail[i]
            if ch == '\x1b':
                m = re.match(r'\x1b(\[[0-9;?<>]*[ -/]*[@-~]|\][^\x07\x1b]*(\x07|\x1b\\)|[()][0-9A-Za-z]|[=>78])', nonlocal_tail[i:])
                if not m:
                    break  # incomplete: wait for more bytes
                seq = m.group(0)
                if seq.endswith('H') and seq.startswith('\x1b['):
                    parts = (seq[2:-1].split(';') + ['', ''])[:2]
                    cur[0] = max(int(parts[0] or 1) - 1, 0)
                    cur[1] = max(int(parts[1] or 1) - 1, 0)
                elif seq in ('\x1b[2J',):
                    for r in grid:
                        r[:] = [' '] * COLS
                elif seq.startswith('\x1b[') and seq.endswith('K'):
                    if cur[0] < ROWS:
                        grid[cur[0]][cur[1]:] = [' '] * (COLS - cur[1])
                i += len(seq)
                continue
            if ch == '\r':
                cur[1] = 0
            elif ch == '\n':
                cur[0] += 1
            elif ch >= ' ':
                if cur[0] < ROWS and cur[1] < COLS:
                    grid[cur[0]][cur[1]] = ch
                cur[1] += 1
            i += 1
        feed.tail = nonlocal_tail[i:]

    feed.tail = ''

    def pump(secs=0.6):
        end = time.time() + secs
        while time.time() < end:
            r, _, _ = select.select([fd], [], [], 0.05)
            if r:
                try:
                    feed(os.read(fd, 65536).decode('utf-8', 'replace'))
                except OSError:
                    return

    def sidebar():
        return [''.join(r[:SIDEBAR]).rstrip() for r in grid]

    def row_of(text):
        return next((i for i, line in enumerate(sidebar()) if text in line), None)

    def order():
        """Space names in the order the sidebar lists them."""
        found = [(row_of(n), n) for n in NAMES]
        return [n for r, n in sorted((r, n) for r, n in found if r is not None)]

    def click(row, col, button=0):
        for end in 'Mm':
            os.write(fd, f'\x1b[<{button};{col + 1};{row + 1}{end}'.encode())
        pump(0.8)

    pump(2.0)
    ok(order() == NAMES, f'manual sort = creation order: {order()}')
    ok(row_of('⇅ manual') is not None, 'header shows the sort')

    # Recent: look at bravo, then alpha → alpha, bravo lead.
    for n in ('bravo', 'alpha'):
        C('workspace', 'focus', str(ids[n]))
        pump(0.8)
    click(row_of('⇅ manual'), 3)
    ok(row_of('⇅ recent') is not None, 'click on the header cycles to recent')
    ok(order()[:2] == ['alpha', 'bravo'], f'recent sort: {order()}')

    click(row_of('⇅ recent'), 3)
    ok(row_of('⇅ a–z') is not None and order() == ['alpha', 'bravo', 'charlie'], f'a–z sort: {order()}')

    # Favorite charlie from its right-click menu: it jumps on top with a ★.
    click(row_of('charlie'), 3, button=2)
    menu_row = next((i for i, r in enumerate(grid) if '★ favorite' in ''.join(r)), None)
    ok(menu_row is not None, 'space menu offers ★ favorite')
    if menu_row is not None:
        click(menu_row, ''.join(grid[menu_row]).index('★ favorite') + 2)
    ok(order()[0] == 'charlie' and '★' in sidebar()[row_of('charlie')], f'favorite pinned on top: {order()}')

    # Filter: prefix+f, type — the list narrows live.
    os.write(fd, b'\x02')
    os.write(fd, b'f')
    pump(0.4)
    for ch in 'brav':
        os.write(fd, ch.encode())
    pump(0.8)
    ok(order() == ['bravo'], f'live filter while typing: {order()}')
    os.write(fd, b'\r')
    pump(0.8)
    frow = row_of('⌕ brav')
    ok(frow is not None and order() == ['bravo'], 'filter stays after Enter, row shows it')
    if frow is not None:
        click(frow, SIDEBAR - 2)
    ok(row_of('⌕ brav') is None and len(order()) == 3, f'✕ clears the filter: {order()}')

    # Sort + favorite persist in the snapshot (autosave).
    time.sleep(6)
    snaps = glob.glob(f'{D}/**/session-*.json', recursive=True)
    snap = json.load(open(snaps[0])) if snaps else {}
    favs = [w['name'] for w in snap.get('workspaces', []) if w.get('favorite')]
    ok(snap.get('space_sort') == 'name' and favs == ['charlie'], f'snapshot keeps sort + favorite: {snap.get("space_sort")} {favs}')
finally:
    if client_pid:
        try:
            os.kill(client_pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    srv.kill()
    srv.wait()
    subprocess.run(['rm', '-rf', D])

print('---')
print(f'pass={passed} fail={failed}')
sys.exit(1 if failed else 0)

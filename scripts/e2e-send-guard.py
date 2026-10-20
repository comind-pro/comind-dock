#!/usr/bin/env python3
"""Sandboxed end-to-end check of "which session am I typing into" UI:
the send guard, the frame header, the task line. Drives a REAL client in a
pty against a throwaway cdock-dev server (temp XDG_STATE_HOME) — never the
live session.

    cargo build && ln -sf cdock target/debug/cdock-dev
    python3 scripts/e2e-send-guard.py
"""
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


def wait_for(fn, timeout=15.0):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(0.25)
    return None


srv = subprocess.Popen([BIN, '--server'], env=ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
client_pid = None
try:
    time.sleep(1)
    # Two fake "claude" sessions: a shell hosting `cat` with claude's OSC
    # title is detected as a claude agent (title fallback while a child runs).
    first = C('pane', 'list')['panes'][0]['id']
    second = C('pane', 'split', str(first), '--direction', 'right')['pane']
    for p in (first, second):
        wait_for(lambda p=p: C('wait', 'agent-status', str(p), '--status', 'idle', '--timeout', '1000').get('ok'), 30)
        C('pane', 'run', str(p), "printf '\\033]0;✳ Claude Code\\007'; stty -echo; cat")
    agents = wait_for(lambda: all(r.get('agent') == 'claude' for r in C('pane', 'list')['panes'] if r['id'] in (first, second)))
    ok(agents, 'both panes detected as claude sessions')

    # A real client in a pty, 160x45.
    client_pid, fd = pty.fork()
    if client_pid == 0:
        os.execve(BIN, [BIN], ENV)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack('HHHH', 45, 160, 0, 0))
    os.kill(client_pid, signal.SIGWINCH)
    screen = bytearray()

    def pump(secs=0.6):
        end = time.time() + secs
        while time.time() < end:
            r, _, _ = select.select([fd], [], [], 0.05)
            if r:
                try:
                    screen.extend(os.read(fd, 65536))
                except OSError:
                    return

    def seen(text, since=0):
        plain = re.sub(rb'\x1b\[[0-9;?]*[A-Za-z]', b'', bytes(screen[since:])).decode('utf-8', 'replace')
        return text in plain

    def typed(text):
        for ch in text:
            os.write(fd, ch.encode())
        pump(0.4)

    def lines_of(p):
        return subprocess.run([BIN, 'pane', 'read', str(p), '--plain', '--lines', '40'], env=ENV,
                              capture_output=True, text=True).stdout

    pump(1.5)
    ok(seen('› ') and seen(f'%{second}'), 'frame header names space › session · %id')

    # 1. First real message to a session: Enter is held, banner names it.
    C('pane', 'focus', str(first))
    pump()
    mark = len(screen)
    typed('please review the auth diff')
    os.write(fd, b'\r')
    pump(0.8)
    ok(seen('↵ again →', mark), 'first Enter held with banner')
    ok('please review the auth diff' not in lines_of(first), 'nothing submitted while held')
    os.write(fd, b'\r')
    pump(0.8)
    ok('please review the auth diff' in lines_of(first), 'second Enter sends')

    # 2. Same session again: no question.
    mark = len(screen)
    typed('and check the token expiry')
    os.write(fd, b'\r')
    pump(0.8)
    ok(not seen('↵ again →', mark) and 'and check the token expiry' in lines_of(first),
       'same session: Enter sends at once')

    # 3. Switch session, Esc cancels: nothing sent, draft stays for the user.
    C('pane', 'focus', str(second))
    pump()
    mark = len(screen)
    typed('wrong window comment here')
    os.write(fd, b'\r')
    pump(0.8)
    ok(seen('↵ again →', mark), 'switched session: Enter held')
    os.write(fd, b'\x1b')
    pump(0.8)
    ok('wrong window comment here' not in lines_of(second), 'Esc cancels the send')

    # 4. Short prompt answers flow without a question.
    C('pane', 'focus', str(first))
    pump()
    os.write(fd, b'\x15')  # ctrl-u clears our draft mirror
    mark = len(screen)
    typed('y')
    os.write(fd, b'\r')
    pump(0.8)
    ok(not seen('↵ again →', mark), 'short answer ("y") never asks')

    # 5. The sidebar shows what each session is about.
    pump(1.0)
    # The sidebar is narrow: the task is truncated there, so match its head.
    ok(seen('› and check the') or seen('› please review'), 'sidebar task line')
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

"""Exercise the real binary inside disposable terminals, never the user's TTY."""
import contextlib
import codecs
import fcntl
import os
from pathlib import Path
import select
import re
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import unittest
import unicodedata


BINARY = str(Path(sys.argv.pop(1)).resolve()) if len(sys.argv) > 1 else str(Path("target/debug/sbar").resolve())


def read_until(fd, marker, timeout=5):
    output = bytearray()
    deadline = time.monotonic() + timeout
    while marker not in output and time.monotonic() < deadline:
        if select.select([fd], [], [], 0.02)[0]:
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                break
            if not chunk:
                break
            output.extend(chunk)
    return bytes(output)


def attributes(fd):
    result = termios.tcgetattr(fd)
    # macOS may set this transient kernel flag when pending input is reprocessed.
    result[3] &= ~getattr(termios, "PENDIN", 0)
    return result


class Session:
    def __init__(self, shell, env=None, nonblocking=False, size=(24, 80)):
        self.master, self.slave = os.openpty()
        if nonblocking:
            fcntl.fcntl(self.slave, fcntl.F_SETFL, fcntl.fcntl(self.slave, fcntl.F_GETFL) | os.O_NONBLOCK)
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", *size, 0, 0))
        self.original = attributes(self.slave)
        self.original_flags = fcntl.fcntl(self.slave, fcntl.F_GETFL)
        status_read, status_write = os.pipe()

        def setup():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)

        # Keep the outer session alive after sbar exits, just like a real shell.
        # Otherwise macOS revokes the slave and termios cannot be inspected.
        keeper_code = (
            "import os,subprocess,sys,time\n"
            "status=os.fdopen(int(sys.argv[2]),'w',buffering=1)\n"
            "child=subprocess.Popen([sys.argv[1]])\n"
            "status.write(str(child.pid)+'\\n')\n"
            "status.write(str(child.wait())+'\\n')\n"
            "time.sleep(30)\n"
        )
        child_env = dict(os.environ, SHELL=str(shell), TERM="xterm-256color")
        child_env.update(env or {})
        self.keeper = subprocess.Popen(
            [sys.executable, "-c", keeper_code, BINARY, str(status_write)],
            stdin=self.slave, stdout=self.slave, stderr=self.slave,
            preexec_fn=setup, pass_fds=(status_write,), env=child_env,
        )
        os.close(status_write)
        self.status = os.fdopen(status_read, "rb", buffering=0)
        if not select.select([self.status], [], [], 5)[0]:
            raise AssertionError("terminal keeper failed to start")
        self.pid = int(self.status.readline())
        self.exit_code = None

    def wait(self, timeout=5):
        if self.exit_code is None:
            if not select.select([self.status], [], [], timeout)[0]:
                raise AssertionError("sbar did not exit in time")
            self.exit_code = int(self.status.readline())
        return self.exit_code

    def close(self):
        try:
            if self.exit_code is None:
                with contextlib.suppress(ProcessLookupError):
                    os.kill(self.pid, signal.SIGTERM)
                self.wait()
        finally:
            termios.tcsetattr(self.slave, termios.TCSANOW, self.original)
            # Close the outer PTY before waiting: macOS can otherwise wait for
            # unread terminal output to drain while a killed helper exits.
            self.keeper.kill()
            os.close(self.master)
            os.close(self.slave)
            self.keeper.wait(timeout=5)
            self.status.close()


class Screen:
    """Small independent VT screen model for cursor/scroll-region assertions.

    DECSTBM homes the cursor; DECSC/DECRC save and restore it. Testing resulting
    cells catches misplaced shell text even when all expected bytes are present.
    """
    def __init__(self, rows=24, cols=80):
        self.rows, self.cols = rows, cols
        self.cells = [[" "] * cols for _ in range(rows)]
        self.x = self.y = 0
        self.saved = (0, 0)
        self.top, self.bottom = 0, rows - 1
        self.pending = ""
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self.normal_buffer = None
        self.frame_open = False

    def feed(self, data):
        text = self.pending + self.decoder.decode(data)
        self.pending = ""
        i = 0
        while i < len(text):
            char = text[i]
            if char == "\x1b":
                start = i
                if i + 1 >= len(text):
                    self.pending = text[start:]
                    break
                command = text[i + 1]
                i += 2
                if command == "7":
                    self.saved = (self.x, self.y)
                    self.frame_open = True
                elif command == "8":
                    self.x, self.y = self.saved
                    self.frame_open = False
                elif command == "D":
                    self.newline()
                elif command == "]":
                    end = text.find("\x07", i)
                    if end == -1:
                        self.pending = text[start:]
                        break
                    i = end + 1
                elif command == "[":
                    parameters = i
                    while i < len(text) and not ("@" <= text[i] <= "~"):
                        i += 1
                    if i == len(text):
                        self.pending = text[start:]
                        break
                    values = text[parameters:i]
                    command = text[i]
                    i += 1
                    if values.startswith("?"):
                        if "1049" in values[1:].split(";"):
                            if command == "h" and self.normal_buffer is None:
                                self.normal_buffer = (self.cells, self.x, self.y, self.saved, self.top, self.bottom)
                                self.cells = [[" "] * self.cols for _ in range(self.rows)]
                                self.x = self.y = 0
                                self.saved = (0, 0)
                                self.top, self.bottom = 0, self.rows - 1
                            elif command == "l" and self.normal_buffer is not None:
                                self.cells, self.x, self.y, self.saved, self.top, self.bottom = self.normal_buffer
                                self.normal_buffer = None
                        continue
                    args = [int(value or 0) for value in values.split(";")]
                    if command in ("H", "f"):
                        self.y = min(self.rows - 1, max(0, (args[0] or 1) - 1))
                        self.x = min(self.cols - 1, max(0, (args[1] if len(args) > 1 else 1) - 1))
                    elif command == "r":
                        self.top = max(0, (args[0] or 1) - 1)
                        self.bottom = min(self.rows - 1, (args[1] if len(args) > 1 else self.rows) - 1)
                        self.x = self.y = 0
                    elif command == "A":
                        self.y = max(self.top, self.y - (args[0] or 1))
                    elif command == "K":
                        start_col = 0 if args[0] == 2 else self.x
                        for x in range(start_col, self.cols):
                            self.cells[self.y][x] = " "
                    elif command == "J" and args[0] == 2:
                        self.cells = [[" "] * self.cols for _ in range(self.rows)]
                continue
            i += 1
            if char == "\r":
                self.x = 0
            elif char == "\n":
                self.newline()
            elif char == "\b":
                self.x = max(0, self.x - 1)
            elif char >= " " and not unicodedata.combining(char):
                width = 2 if unicodedata.east_asian_width(char) in ("W", "F") else 1
                if self.x + width > self.cols:
                    self.x = 0
                    self.newline()
                self.cells[self.y][self.x] = char
                self.x += width

    def newline(self):
        if self.y == self.bottom:
            del self.cells[self.top]
            self.cells.insert(self.bottom, [" "] * self.cols)
        else:
            self.y = min(self.rows - 1, self.y + 1)

    def resize(self, rows, cols):
        # Preserve the active line when shrinking vertically, as a terminal does
        # when it scrolls upper content away to keep its cursor visible.
        if self.y >= rows:
            removed = self.y - rows + 1
            self.cells = self.cells[removed:]
            self.y -= removed
        self.cells = [(row + [" "] * cols)[:cols] for row in self.cells[:rows]]
        self.cells.extend([[" "] * cols for _ in range(rows - len(self.cells))])
        self.rows, self.cols = rows, cols
        self.x, self.y = min(self.x, cols - 1), min(self.y, rows - 1)
        self.top, self.bottom = 0, rows - 1


def feed_screen(session, screen, output):
    screen.feed(output)
    deadline = time.monotonic() + 5
    # A timed read can stop halfway through painting a row. Assert rendered
    # positions only after the renderer has restored the shell cursor.
    while screen.pending or screen.frame_open or screen.decoder.getstate()[0]:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise AssertionError("terminal frame did not finish")
        chunk = read_until(session.master, b"\x1b8", timeout=remaining)
        if not chunk:
            raise AssertionError("terminal frame did not finish")
        screen.feed(chunk)


class PtyTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="sbar-test-")
        self.root = Path(self.temporary.name)
        self.addCleanup(self.temporary.cleanup)

    def script(self, name, body):
        path = self.root / name
        path.write_text("#!" + sys.executable + "\n" + body)
        path.chmod(0o755)
        return path

    def session(self, shell, env=None, nonblocking=False, size=(24, 80)):
        session = Session(shell, env, nonblocking, size)
        self.addCleanup(session.close)
        return session

    def assert_restored(self, session):
        self.assertEqual(attributes(session.slave), session.original)
        self.assertEqual(fcntl.fcntl(session.slave, fcntl.F_GETFL), session.original_flags)

    def sync_screen_size(self, session, screen, cols):
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            os.write(session.master, b"s")
            output = read_until(session.master, b"SIZEDONE\x07")
            feed_screen(session, screen, output)
            match = re.search(rb"SIZE:(\d+):(\d+):SIZEDONE", output)
            self.assertIsNotNone(match, "shell did not report its PTY dimensions")
            if int(match[2]) == cols:
                return
        self.fail("shell did not receive new terminal dimensions")

    def test_normal_exit_preserves_output_and_status(self):
        shell = self.script("exit-shell", "import sys\nprint('FINAL_MARKER',flush=True)\nsys.exit(7)\n")
        session = self.session(shell)
        self.assertIn(b"FINAL_MARKER", read_until(session.master, b"FINAL_MARKER"))
        self.assertEqual(session.wait(), 7)
        self.assert_restored(session)

    def test_signals_restore_terminal_and_reap_shell(self):
        pid_file = self.root / "shell.pid"
        shell = self.script("idle-shell", "import os,time\nfrom pathlib import Path\n"
                            "Path(os.environ['SBAR_TEST_PID']).write_text(str(os.getpid()))\n"
                            "print('READY',flush=True)\ntime.sleep(30)\n")
        for sig in (signal.SIGTERM, signal.SIGHUP):
            with self.subTest(signal=sig):
                session = self.session(shell, {"SBAR_TEST_PID": str(pid_file)})
                self.assertIn(b"READY", read_until(session.master, b"READY"))
                self.assertFalse(attributes(session.slave)[3] & termios.ICANON)
                child_pid = int(pid_file.read_text())
                os.kill(session.pid, sig)
                self.assertEqual(session.wait(), 128 + sig)
                self.assert_restored(session)
                with self.assertRaises(ProcessLookupError):
                    os.kill(child_pid, 0)

    def test_slow_gpu_does_not_block_input_and_is_reaped(self):
        probe_pid = self.root / "probe.pid"
        probe = self.script("ioreg" if sys.platform == "darwin" else "nvidia-smi",
                            "import os,time\nfrom pathlib import Path\n"
                            "Path(os.environ['SBAR_TEST_PROBE_PID']).write_text(str(os.getpid()))\n"
                            "time.sleep(30)\n")
        shell = self.script("echo-shell", "import sys,time\nprint('READY',flush=True)\n"
                            "line=sys.stdin.readline()\nprint('RESULT:'+line,flush=True)\ntime.sleep(30)\n")
        session = self.session(shell, {
            "PATH": str(probe.parent) + os.pathsep + os.environ["PATH"],
            "SBAR_TEST_PROBE_PID": str(probe_pid),
        })
        self.assertIn(b"READY", read_until(session.master, b"READY"))
        deadline = time.monotonic() + 5
        while not probe_pid.exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertTrue(probe_pid.exists(), "GPU probe was never started")
        gpu_pid = int(probe_pid.read_text())
        os.write(session.master, b"hello\n")
        self.assertIn(b"RESULT:hello", read_until(session.master, b"RESULT:hello", timeout=0.5))
        # Interrupt while the probe is still running; cleanup must cancel/reap it.
        os.kill(session.pid, signal.SIGTERM)
        self.assertEqual(session.wait(), 128 + signal.SIGTERM)
        self.assert_restored(session)
        with self.assertRaises(ProcessLookupError):
            os.kill(gpu_pid, 0)

    def test_failed_shell_exec_restores_terminal(self):
        session = self.session(self.root / "does-not-exist")
        self.assertEqual(session.wait(), 127)
        self.assert_restored(session)

    def test_blocked_output_signals_restore_terminal_without_reader(self):
        pid_file = self.root / "flood.pid"
        shell = self.script("flood-shell", "import os,time\nfrom pathlib import Path\n"
                            "Path(os.environ['SBAR_TEST_PID']).write_text(str(os.getpid()))\n"
                            "print('READY',flush=True)\ntime.sleep(0.1)\n"
                            "while True: os.write(1,b'x'*16384)\n")
        for sig, nonblocking in ((signal.SIGTERM, False), (signal.SIGHUP, True)):
            with self.subTest(signal=sig, nonblocking=nonblocking):
                session = self.session(shell, {"SBAR_TEST_PID": str(pid_file)}, nonblocking)
                self.assertIn(b"READY", read_until(session.master, b"READY"))
                child_pid = int(pid_file.read_text())
                time.sleep(0.5)  # Fill the outer PTY and sbar's bounded queue.
                self.assertEqual(fcntl.fcntl(session.slave, fcntl.F_GETFL), session.original_flags)
                os.kill(session.pid, sig)
                # Do not drain any output before checking restoration and exit.
                self.assertEqual(session.wait(timeout=2), 128 + sig)
                self.assert_restored(session)
                with self.assertRaises(ProcessLookupError):
                    os.kill(child_pid, 0)

    def test_normal_exit_drains_large_output_without_loss(self):
        payload = bytes(range(32, 127)) * 8192
        shell = self.script("large-output-shell", "import os,sys,tty\ntty.setraw(0)\n"
                            "os.write(1,b'\\x1b[?1049hALT_READY')\nos.read(0,1)\n"
                            "data=b'PAYLOAD_BEGIN'+bytes(range(32,127))*8192+b'PAYLOAD_END'\n"
                            "while data:\n n=os.write(1,data)\n data=data[n:]\n"
                            "sys.exit(17)\n")
        session = self.session(shell)
        self.assertIn(b"ALT_READY", read_until(session.master, b"ALT_READY"))
        read_until(session.master, b"NEVER", timeout=0.1)
        os.write(session.master, b"x")
        time.sleep(0.5)
        output = read_until(session.master, b"PAYLOAD_END", timeout=10)
        self.assertIn(b"PAYLOAD_BEGIN", output)
        actual = output.split(b"PAYLOAD_BEGIN", 1)[1].split(b"PAYLOAD_END", 1)[0]
        self.assertEqual(actual, payload)
        self.assertEqual(session.wait(), 17)
        self.assert_restored(session)

    def test_cursor_position_survives_bar_resize_and_exit(self):
        shell = self.script("positioned-shell", "import os,tty\ntty.setraw(0)\n"
                            "os.write(1,b'\\x1b[10;15HPROMPT')\n"
                            "for word in (b'AFTER',b'RESIZE',b'FINAL'):\n"
                            " os.read(0,1)\n os.write(1,word)\n")
        session = self.session(shell)
        screen = Screen()
        feed_screen(session, screen, read_until(session.master, b"PROMPT"))
        first_bar = read_until(session.master, b"CPU", timeout=4)
        self.assertIn(b"CPU", first_bar)
        feed_screen(session, screen, first_bar)
        # Finish reading the frame if the terminal split it at the marker.
        feed_screen(session, screen, read_until(session.master, b"NEVER", timeout=0.1))
        os.write(session.master, b"x")
        feed_screen(session, screen, read_until(session.master, b"AFTER"))
        self.assertIn("PROMPTAFTER", "".join(screen.cells[9]))
        # Resize both the model and real PTY; the wider layout can also change
        # bar height. Shell output must still continue at its previous cursor.
        screen.resize(26, 240)
        fcntl.ioctl(session.slave, termios.TIOCSWINSZ, struct.pack("HHHH", 26, 240, 0, 0))
        os.kill(session.pid, signal.SIGWINCH)
        feed_screen(session, screen, read_until(session.master, b"NEVER", timeout=0.15))
        os.write(session.master, b"x")
        feed_screen(session, screen, read_until(session.master, b"RESIZE"))
        self.assertIn("PROMPTAFTERRESIZE", "".join(screen.cells[9]))
        os.write(session.master, b"x")
        feed_screen(session, screen, read_until(session.master, b"FINAL"))
        self.assertEqual(session.wait(), 0)
        feed_screen(session, screen, read_until(session.master, b"NEVER", timeout=0.1))
        self.assertIn("PROMPTAFTERRESIZEFINAL", "".join(screen.cells[9]))
        self.assertEqual((screen.x, screen.y), (14 + len("PROMPTAFTERRESIZEFINAL"), 9))
        self.assert_restored(session)

    def test_alternate_screen_suspends_bar_and_uses_full_size(self):
        shell = self.script("alternate-shell", "import os,time,tty\ntty.setraw(0)\n"
                            "os.write(1,b'\\x1b[24;15HPROMPT')\nos.read(0,1)\n"
                            "os.write(1,b'\\x1b[?10')\ntime.sleep(1.1)\n"
                            "os.write(1,b'49hALT_READY')\nos.read(0,1)\n"
                            "os.write(1,('ROWS:'+str(os.get_terminal_size(0).lines)).encode())\n"
                            "os.read(0,1)\nos.write(1,b'\\x1b[?1049lNORMAL')\nos.read(0,1)\n"
                            "os.write(1,b'DONE')\n")
        session = self.session(shell)
        screen = Screen()
        output = read_until(session.master, b"CPU", timeout=4)
        self.assertIn(b"CPU", output)
        feed_screen(session, screen, output)
        feed_screen(session, screen, read_until(session.master, b"NEVER", timeout=0.1))
        prompt_position = (screen.x, screen.y)
        self.assertIn("PROMPT", "".join(screen.cells[screen.y]))
        os.write(session.master, b"x")
        output = read_until(session.master, b"ALT_READY")
        self.assertIn(b"\x1b[?1049hALT_READY", output)
        feed_screen(session, screen, output)
        output = read_until(session.master, b"NEVER", timeout=1.2)
        self.assertNotIn(b"CPU", output)
        feed_screen(session, screen, output)
        os.write(session.master, b"x")
        output = read_until(session.master, b"ROWS:24")
        self.assertIn(b"ROWS:24", output)
        feed_screen(session, screen, output)
        os.write(session.master, b"x")
        output = read_until(session.master, b"CPU", timeout=4)
        self.assertIn(b"CPU", output)
        feed_screen(session, screen, output)
        feed_screen(session, screen, read_until(session.master, b"NEVER", timeout=0.1))
        self.assertIn("PROMPTNORMAL", "".join(screen.cells[screen.y]))
        self.assertEqual((screen.x, screen.y),
                         (prompt_position[0] + len("NORMAL"), min(prompt_position[1], screen.bottom)))
        os.write(session.master, b"x")
        self.assertIn(b"DONE", read_until(session.master, b"DONE"))
        self.assertEqual(session.wait(), 0)
        self.assert_restored(session)

    def test_bottom_prompt_survives_reservation_resize_and_repaints(self):
        # Eight mocked GPUs take one/two wide rows but eight compact rows. This
        # makes a width-only resize grow the bar without discarding screen cells.
        gate = self.root / "prompt-forwarded"
        probe = self.script("ioreg" if sys.platform == "darwin" else "nvidia-smi",
                            "import os,sys,time\nfrom pathlib import Path\n"
                            "while not Path(os.environ['SBAR_TEST_GATE']).exists(): time.sleep(0.001)\n"
                            "print(('\"PerformanceStatistics\" = {\"Device Utilization %\"=1}\\n'"
                            " if sys.platform=='darwin' else 'GPU,1,1,1024\\n')*8)\n")
        shell = self.script("bottom-shell", "import os,tty\ntty.setraw(0)\n"
                            "os.write(1,b'\\x1b[24;15HPROMPT')\n"
                            "while os.read(0,1)==b's':\n"
                            " size=os.get_terminal_size(0)\n"
                            " os.write(1,('\\x1b]0;SIZE:%d:%d:SIZEDONE\\x07'%(size.lines,size.columns)).encode())\n"
                            "os.write(1,b'FINAL')\n")
        session = self.session(shell, {"PATH": str(probe.parent) + os.pathsep + os.environ["PATH"],
                                       "SBAR_TEST_GATE": str(gate)}, size=(24, 600))
        screen = Screen(cols=600)
        output = read_until(session.master, b"PROMPT")
        self.assertIn(b"PROMPT", output)
        feed_screen(session, screen, output)
        gate.touch()  # The first GPU sample follows the already-forwarded prompt.
        output = read_until(session.master, b"CPU", timeout=4)
        self.assertIn(b"CPU", output)
        feed_screen(session, screen, output)
        feed_screen(session, screen, read_until(session.master, b"NEVER", timeout=0.1))
        self.assertEqual(screen.y, screen.bottom)
        self.assertEqual(screen.x, 14 + len("PROMPT"))
        self.assertIn("PROMPT", "".join(screen.cells[screen.y]))
        for _ in range(2):
            previous_y = screen.y
            feed_screen(session, screen, read_until(session.master, b"NEVER", timeout=1.1))
            # A genuinely taller metric layout can move the prompt up once;
            # repainting an unchanged layout must not accumulate scrolling.
            self.assertEqual(screen.y, min(previous_y, screen.bottom))
            self.assertEqual(screen.x, 14 + len("PROMPT"))
            self.assertIn("PROMPT", "".join(screen.cells[screen.y]))
        previous_bottom = screen.bottom
        screen.resize(24, 60)
        fcntl.ioctl(session.slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 60, 0, 0))
        os.kill(session.pid, signal.SIGWINCH)
        self.sync_screen_size(session, screen, 60)
        self.assertLess(screen.bottom, previous_bottom, "narrow layout must reserve more GPU rows")
        self.assertEqual(screen.y, screen.bottom)
        self.assertIn("PROMPT", "".join(screen.cells[screen.y]))
        screen.resize(12, 80)
        fcntl.ioctl(session.slave, termios.TIOCSWINSZ, struct.pack("HHHH", 12, 80, 0, 0))
        os.kill(session.pid, signal.SIGWINCH)
        self.sync_screen_size(session, screen, 80)
        self.assertLessEqual(screen.y, screen.bottom)
        self.assertIn("PROMPT", "".join(screen.cells[screen.y]))
        for _ in range(2):
            previous_y = screen.y
            feed_screen(session, screen, read_until(session.master, b"NEVER", timeout=1.1))
            self.assertEqual(screen.y, min(previous_y, screen.bottom))
            self.assertIn("PROMPT", "".join(screen.cells[screen.y]))
        os.write(session.master, b"x")
        feed_screen(session, screen, read_until(session.master, b"FINAL"))
        self.assertIn("PROMPTFINAL", "".join(screen.cells[screen.y]))
        self.assertEqual(session.wait(), 0)
        self.assert_restored(session)


if __name__ == "__main__":
    unittest.main()

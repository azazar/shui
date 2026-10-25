"""Exercise the real TUI and command handoff without making network connections."""

import errno
import faulthandler
import fcntl
import os
from pathlib import Path
import pty
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

# Shift+F4 opens Add new directly, focused on its blank remote.
ADD_REMOTE = b"\x1b[1;2S"
# Wait for the external command to relinquish the terminal and the UI to resume.
RETURNED = (b"\x1b[?1049l", b"\x1b[?1049h")


def read_pty(master):
    """PTY closure is EOF on macOS and EIO on Linux."""
    try:
        return os.read(master, 65536)
    except OSError as error:
        if error.errno == errno.EIO:
            return b""
        raise


def wait_pty(process, master, timeout, consume):
    """Keep draining output while waiting, including terminal teardown writes."""
    deadline = time.monotonic() + timeout
    while True:
        status = process.poll()
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise subprocess.TimeoutExpired(process.args, timeout)
        if select.select(
            [master], [], [], 0 if status is not None else min(remaining, 0.05)
        )[0]:
            chunk = read_pty(master)
            if chunk:
                consume(chunk)
                continue
        if status is not None:
            return status


def close_pty(process, master, slave):
    # Closing the terminal before reaping also releases a killed session leader
    # blocked in terminal teardown on macOS.
    if process.poll() is None:
        process.kill()
    os.close(master)
    os.close(slave)
    process.wait(timeout=3)


def check_pty_drain():
    """Output larger than the PTY buffer must not deadlock exit or cleanup."""
    for finish in [True, False]:
        master, slave = pty.openpty()
        process = subprocess.Popen(
            [sys.executable, "-c", "import os; os.write(1, b'x' * 262144)"],
            stdin=slave,
            stdout=slave,
            stderr=slave,
        )
        try:
            if finish:
                output = bytearray()
                assert wait_pty(process, master, 5, output.extend) == 0
                assert output == b"x" * 262144
            else:
                assert select.select([master], [], [], 5)[0], "writer did not start"
                assert process.poll() is None, "writer did not fill the PTY buffer"
        finally:
            close_pty(process, master, slave)


def interaction_checks(binary):
    """Assert the final screen and geometry after real keyboard/focus/resize events."""
    import pyte

    with tempfile.TemporaryDirectory(prefix="shui-interaction-") as directory:
        root = Path(directory)
        (root / ".ssh").mkdir()
        hosts = [f"host-{index:02}-long-production-name" for index in range(45)]
        (root / ".ssh/config").write_text(
            "".join(
                f"Host {host}\n User tester\n HostName 192.0.2.{index + 1}\n"
                " IdentityFile ~/.ssh/a-long-private-key-name\n ProxyJump jump.example.org\n"
                for index, host in enumerate(hosts)
            )
        )
        config = root / "settings/shui.yml"
        config.parent.mkdir(parents=True)
        # The harmless commands are deliberate: a regression must never use real SSH.
        profiles = """  - {name: Zero, key: Alt+Z, command: 'true %h'}
  - name: One
    key: Ctrl+Alt+O
    command: 'true %h %{input:first}'
    inputs: [{id: first, type: promptString, default: first-value}]
  - name: Many
    command: 'true %h %{input:first} %{input:second} %{input:third} %{input:fourth}'
    inputs:
      - {id: first, type: promptString, default: first-value}
      - {id: second, type: promptString, default: second-value}
      - {id: third, type: promptString, default: third-value}
      - {id: fourth, type: promptString, default: fourth-value}
"""
        recipes = profiles.replace("key: Alt+Z, ", "").replace(
            "    key: Ctrl+Alt+O\n", ""
        )
        config.write_text(
            "default: Zero\nprofiles:\n"
            + profiles
            + "configuration_scripts:\n"
            + recipes
        )
        env = dict(
            os.environ,
            HOME=str(root),
            USER="tester",
            TERM="xterm-256color",
            XDG_CONFIG_HOME=str(root / "settings"),
        )
        hosts = subprocess.check_output(
            [str(binary), "--dump"], env=env, text=True
        ).splitlines()

        class ScreenSession:
            def __init__(self, rows, cols):
                self.master, self.slave = pty.openpty()
                self.rows, self.cols = rows, cols
                self.screen = pyte.Screen(cols, rows)
                self.stream = pyte.ByteStream(self.screen)
                fcntl.ioctl(
                    self.slave,
                    termios.TIOCSWINSZ,
                    struct.pack("HHHH", rows, cols, 0, 0),
                )
                self.before = termios.tcgetattr(self.slave)
                self.process = subprocess.Popen(
                    [str(binary)],
                    stdin=self.slave,
                    stdout=self.slave,
                    stderr=self.slave,
                    env=env,
                    cwd=root,
                )

            def __enter__(self):
                try:
                    deadline = time.monotonic() + 5
                    while (
                        "SHUI" not in self.screen.display[0]
                        and time.monotonic() < deadline
                    ):
                        self.drain()
                        if self.process.poll() is not None:
                            break
                    self.require("SHUI" in self.screen.display[0], "menu did not open")
                    return self
                except BaseException:
                    self.__exit__(*sys.exc_info())
                    raise

            def __exit__(self, *error):
                try:
                    # Cancel a form/help first, then exit the menu. Never send Enter.
                    for _ in range(3):
                        if self.process.poll() is not None:
                            break
                        self.send(b"\x03")
                    wait_pty(self.process, self.master, 2, self.stream.feed)
                    if error[0] is None:
                        self.require(self.process.returncode == 0, "unexpected exit")
                        assert termios.tcgetattr(self.slave) == self.before
                finally:
                    close_pty(self.process, self.master, self.slave)

            def drain(self):
                # Wait for one complete redraw and a quiet period, with a hard bound.
                deadline = time.monotonic() + 1
                while time.monotonic() < deadline:
                    if not select.select([self.master], [], [], 0.08)[0]:
                        return
                    chunk = read_pty(self.master)
                    if not chunk:
                        return
                    self.stream.feed(chunk)

            def send(self, key):
                os.write(self.master, key)
                self.drain()

            def resize(self, rows, cols):
                self.rows, self.cols = rows, cols
                self.screen.resize(lines=rows, columns=cols)
                fcntl.ioctl(
                    self.slave,
                    termios.TIOCSWINSZ,
                    struct.pack("HHHH", rows, cols, 0, 0),
                )
                self.process.send_signal(signal.SIGWINCH)
                self.drain()

            def require(self, condition, message):
                assert condition, message + "\n" + "\n".join(self.screen.display)

            def locate(self, text):
                found = [
                    (row, line.index(text))
                    for row, line in enumerate(self.screen.display)
                    if text in line
                ]
                self.require(
                    len(found) == 1, f"expected one visible {text!r}, got {found}"
                )
                return found[0]

            def selected(self):
                found = [
                    (row, line)
                    for row, line in enumerate(self.screen.display)
                    if "› " in line
                ]
                self.require(len(found) == 1, f"expected one selected row, got {found}")
                return found[0]

            def focus(self, label):
                for _ in range(12):
                    if label in self.selected()[1]:
                        return
                    self.send(b"\t")
                self.require(False, f"could not focus {label}")

            def footer(self):
                return self.screen.display[-1]

            def normal_bar(self):
                footer = self.footer()
                self.require(
                    "Help" in footer and "Quit" in footer, "normal menu bar missing"
                )
                self.require(
                    not any(text in footer for text in ("Shift", "Ctrl", "Alt", "New")),
                    "normal bar exposed modified shortcuts",
                )

        for rows, cols in [(24, 100), (16, 65), (9, 40), (6, 26)]:
            with ScreenSession(rows, cols) as tui:
                tui.normal_bar()
                tui.send(b"\x1bOS")
                tui.require(
                    "[ ]" in tui.selected()[1] and "Zero" in tui.selected()[1],
                    "F4 did not focus the first checkbox",
                )
                anchors = {label: tui.locate(label) for label in ("REMOTE SETUP",)}
                tui.send(b"\r")  # Validation errors use the reserved status area.
                for label, position in anchors.items():
                    tui.require(
                        tui.locate(label) == position, f"validation moved {label}"
                    )
                tui.focus("One")
                tui.send(b" ")
                tui.focus("Many")
                tui.send(b" ")
                if rows == 24:
                    visible = "\n".join(tui.screen.display)
                    tui.require(
                        visible.count("first-value") == 2 and "fourth-value" in visible,
                        "selected recipes' options are not visible together",
                    )
                    tui.send(b" ")
                    visible = "\n".join(tui.screen.display)
                    tui.require(
                        visible.count("first-value") == 1
                        and "fourth-value" not in visible,
                        "unchecking did not hide only that recipe's options",
                    )
                    tui.send(b" ")
                # Both selected recipes retain their visible input rows.
                tui.focus("One")
                tui.send(b"\t")
                tui.require(
                    "first-value" in tui.selected()[1],
                    "first recipe options disappeared",
                )
                tui.focus("fourth")
                tui.require("fourth-value" in tui.selected()[1], "last input obscured")
                tui.require(
                    tui.locate("REMOTE SETUP") == anchors["REMOTE SETUP"],
                    "scrolling fields moved the header",
                )
                tui.focus("Many")
                tui.send(b"\x1bOP")  # Help returns to the same field and geometry.
                tui.send(b"\x1bOP")
                tui.require("Many" in tui.selected()[1], "help lost form focus")
                tui.require(
                    tui.locate("REMOTE SETUP") == anchors["REMOTE SETUP"],
                    "help moved the dialog",
                )
                tui.send(b"\x1b")
                tui.send(b"\x1b[13;5u")
                tui.focus("Profile")
                anchors = {
                    label: tui.locate(label)
                    for label in ("CONNECT", "Remote", "Profile", "Cancel")
                }
                for name in ("One", "Many", "Zero"):
                    tui.send(b"\x1b[C")
                    tui.require(
                        name in tui.selected()[1], f"profile {name} not selected"
                    )
                    for label, position in anchors.items():
                        tui.require(
                            tui.locate(label) == position, f"{label} moved for {name}"
                        )

        # The form advertises arrows only for fields where they change a choice.
        with ScreenSession(24, 100) as tui:
            tui.send(ADD_REMOTE)
            tui.require("Remote" in tui.selected()[1], "new form did not focus Remote")
            footer_row = tui.locate("Cancel")[0]
            tui.require(
                "Choice" not in tui.screen.display[footer_row], "Remote offered Choice"
            )
            tui.send(b"draft-host")
            before = tui.selected()
            tui.send(b"\x1b[C")
            tui.require(tui.selected() == before, "Right changed the editable Remote")
            tui.focus("Zero")
            tui.require(
                "Queue" in tui.screen.display[footer_row],
                "recipe omitted checkbox action",
            )
            tui.send(b" ")
            tui.require(
                "[x]" in tui.selected()[1] and "Zero" in tui.selected()[1],
                "checkbox did not toggle",
            )

        # Modifier groups are exact. A release keeps the remaining held modifier,
        # and losing window focus resets stale state even if releases were missed.
        for rows, cols in [(24, 100), (10, 40)]:
            with ScreenSession(rows, cols) as tui:
                tui.normal_bar()
                tui.send(b"\x1b[57441;2u")
                tui.require(
                    "Shift" in tui.footer() and "New" in tui.footer(),
                    "Shift menu missing",
                )
                tui.send(b"\x1b[57441;2:3u")
                tui.normal_bar()
                tui.send(b"\x1b[57443;3u")
                tui.require(
                    "Alt" in tui.footer() and "Zero" in tui.footer(),
                    "Alt profile missing",
                )
                tui.send(b"\x1b[57442;7u")
                tui.require(
                    "Ctrl+Alt" in tui.footer() and "One" in tui.footer(),
                    "combined group missing",
                )
                tui.require(
                    "Zero" not in tui.footer(),
                    "Alt-only profile leaked into combination",
                )
                tui.send(b"\x1b[57442;7:3u")
                tui.require(
                    "Alt" in tui.footer() and "Ctrl" not in tui.footer(),
                    "release lost held Alt",
                )
                tui.resize(rows + 2, cols + 3)
                tui.require("Alt" in tui.footer(), "resize lost held modifier")
                tui.send(b"\x1b[O")
                tui.send(b"\x1b[I")
                tui.normal_bar()
                tui.send(b"\x1b[57443;3:3u")
                tui.normal_bar()
                tui.send(b"\x1b?")
                tui.require("Alt" in tui.footer(), "fallback reveal failed")
                tui.send(b"\x1b?")
                tui.normal_bar()
                tui.send(b"\x1b[1;2P")
                tui.require("Shift" in tui.footer(), "Shift fallback reveal failed")
                tui.send(b"\x1b[1;2P")
                tui.normal_bar()
                # Uppercase Shift+Alt is a distinct shortcut, not Alt+Z.
                tui.send(b"\x1b[57443;3u")
                tui.send(b"\x1b[57441;4u")
                for shifted in (b"\x1b[90;4u", b"\x1b[122:90;4u"):
                    tui.send(shifted)
                    tui.require(
                        tui.process.poll() is None, "Shift+Alt+Z launched Alt+Z"
                    )
                    tui.require(
                        "Alt+Shift" in tui.footer() and "Zero" not in tui.footer(),
                        "Shift+Alt offered Alt-only profile",
                    )
                tui.send(b"\x1b[57441;4:3u")
                tui.send(b"\x1b[57442;7u")
                tui.send(b"\x1b[122;7u")
                tui.require(tui.process.poll() is None, "Ctrl+Alt+Z launched Alt+Z")
                tui.send(b"\x1b[57442;7:3u")
                tui.send(b"\x1b[57443;3:3u")
                tui.normal_bar()
                # An invalid unmatched query has no target for any profile action.
                tui.send(b"not a valid address")
                tui.require("Open" not in tui.footer(), "invalid query offered Open")
                tui.require(
                    not any(name in tui.footer() for name in ("Zero", "One", "Many")),
                    "invalid query offered profile cycling",
                )
                tui.send(b"\x1b[57443;3u")
                tui.require(
                    "Zero" not in tui.footer(), "invalid query offered Alt profile"
                )
                tui.send(b"\x1b[57442;7u")
                tui.require(
                    "One" not in tui.footer(), "invalid query offered Ctrl+Alt profile"
                )
                tui.send(b"\x1b[57442;7:3u")
                tui.send(b"\x1b[57443;3:3u")

        # Full lists with long host/config hints must retain a stable viewport and
        # keep the selected host readable, including when refresh shows a notice.
        for rows, cols in [(24, 100), (12, 60), (6, 40)]:
            with ScreenSession(rows, cols) as tui:
                tui.send(b"\x1b[F")
                last_row, last_line = tui.selected()
                tui.require(hosts[-1] in last_line, "End selection obscured")
                first_visible = next(
                    line for line in tui.screen.display[1:] if "host-" in line
                )
                tui.send(b"\x1b[A")
                row, line = tui.selected()
                tui.require(
                    hosts[-2] in line and row == last_row - 1, "Up jumped the viewport"
                )
                tui.require(
                    next(line for line in tui.screen.display[1:] if "host-" in line)
                    == first_visible,
                    "Up changed the first visible host",
                )
                tui.send(b"\x1b[15~")
                tui.require(
                    tui.selected() == (row, line), "refresh moved or obscured selection"
                )
                tui.require("Refreshed" in tui.footer(), "refresh notice not in footer")
                tui.send(b"\x1b[H")
                tui.require(hosts[0] in tui.selected()[1], "Home selection missing")
                tui.send(b"x" * 120 + b"QUERY_TAIL")
                tui.require(
                    "QUERY_TAIL" in tui.screen.display[0], "active query tail clipped"
                )
                tui.resize(rows + 1, cols + 5)
                tui.require(
                    "QUERY_TAIL" in tui.screen.display[0], "resize lost query tail"
                )
                tui.send(b"\x15")
                tui.require(
                    hosts[0] in tui.selected()[1], "Ctrl+U did not restore hosts"
                )
                tui.send(b"\x1b[F")
                tui.require(
                    hosts[-1] in tui.selected()[1],
                    "End selection missing before growth",
                )
                tui.resize(len(hosts) + 4, cols)
                host_lines = tui.screen.display[1 : len(hosts) + 1]
                tui.require(
                    all(host in line for host, line in zip(hosts, host_lines)),
                    "growth retained a stale list offset or omitted hosts",
                )
                tui.require(hosts[-1] in tui.selected()[1], "growth lost selected host")
                tui.resize(rows, cols)
                tui.require(
                    hosts[-1] in tui.selected()[1], "shrink obscured selected host"
                )
    print("terminal screen interactions passed")


faulthandler.dump_traceback_later(60, repeat=True)
source_binary = Path(sys.argv[1]).resolve()
check_pty_drain()
interaction_checks(source_binary)
if sys.argv[2:] == ["--interaction-only"]:
    sys.exit(0)
with tempfile.TemporaryDirectory(prefix="shui-pty-") as directory:
    root = Path(directory)
    # Exercise a single-file installation, with no docs beside it or in the cwd.
    binary = str(root / "shui")
    shutil.copy2(source_binary, binary)
    (root / ".ssh").mkdir()
    (root / ".ssh/id_ed25519").write_text("fake private key")
    (root / ".ssh/id_ed25519.pub").write_text("fake public key")
    (root / ".ssh/id_orphan.pub").write_text("not a pair")
    (root / ".ssh/config").write_text("Host alpha beta\n User tester\n")
    (root / ".bash_history").write_text("ssh alpha\nssh -p 2222 beta\n")
    (root / "bin").mkdir()
    for name in ("ssh", "mosh", "mc", "sftp", "custom-connect"):
        stub = root / "bin" / name
        stub.write_text('#!/bin/sh\nprintf "%s\\n" "$0" "$@" > "$SHUI_TEST_LOG"\n')
        stub.write_text(
            stub.read_text()
            + '[ -z "${SHUI_TEST_DELAY:-}" ] || sleep "$SHUI_TEST_DELAY"\n'
        )
        stub.write_text(
            stub.read_text()
            + 'stty -g > "$SHUI_TEST_LOG.tty"\nprintf "SHUI_HANDOFF\\n"\n'
        )
        stub.chmod(0o755)
    child_env = dict(
        os.environ,
        HOME=str(root),
        USER="tester",
        TERM="xterm",
        PATH=str(root / "bin") + os.pathsep + os.environ["PATH"],
        SHUI_TEST_LOG=str(root / "command"),
        XDG_CONFIG_HOME=str(root / "settings"),
    )
    dump = subprocess.check_output([binary, "--dump"], env=child_env, text=True)
    assert dump.splitlines()[:2] == ["beta:2222", "alpha"], dump

    def startup_failure():
        master, slave = pty.openpty()
        try:
            result = subprocess.run(
                [binary],
                stdin=slave,
                stdout=slave,
                stderr=subprocess.PIPE,
                env=child_env,
                timeout=5,
            )
            output = bytearray()
            while select.select([master], [], [], 0)[0]:
                chunk = read_pty(master)
                if not chunk:
                    break
                output.extend(chunk)
            result.stdout = bytes(output)
            return result
        finally:
            os.close(master)
            os.close(slave)

    def session(
        keys, rows=24, cols=100, expected_status=0, args=(), acknowledge_recipes=True
    ):
        faulthandler.dump_traceback_later(30, repeat=True)
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        before = termios.tcgetattr(slave)
        tty_before = subprocess.check_output(["stty", "-g"], stdin=slave).strip()
        (root / "command.tty").unlink(missing_ok=True)
        (root / "command.setup-tty").unlink(missing_ok=True)
        process = subprocess.Popen(
            [binary, *args],
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=child_env,
            cwd=root,
        )
        output = bytearray()
        pause_prompt = b"Press Enter to continue..."
        pause_cursor = 0

        def receive(chunk):
            nonlocal pause_cursor
            output.extend(chunk)
            if acknowledge_recipes:
                while (position := output.find(pause_prompt, pause_cursor)) >= 0:
                    pause_cursor = position + len(pause_prompt)
                    os.write(master, b"\r")

        def read_for(seconds):
            end = time.monotonic() + seconds
            while time.monotonic() < end:
                if select.select([master], [], [], 0.05)[0]:
                    chunk = read_pty(master)
                    if not chunk:
                        break
                    receive(chunk)

        try:
            deadline = time.monotonic() + 5
            while b"SHUI" not in output:
                assert process.poll() is None, output
                assert time.monotonic() < deadline, output
                read_for(0.05)
            last_key_start = len(output)
            for key in keys:
                if isinstance(key, tuple):
                    deadline = time.monotonic() + 5
                    cursor = last_key_start
                    for marker in key:
                        while (position := output.find(marker, cursor)) < 0:
                            assert process.poll() is None, output
                            assert time.monotonic() < deadline, (
                                marker,
                                bytes(output[last_key_start:]),
                            )
                            read_for(0.05)
                        cursor = position + len(marker)
                    if key == (pause_prompt,) and not acknowledge_recipes:
                        read_for(0.2)
                        assert process.poll() is None, "recipe did not wait for Enter"
                        assert b"\x1b[?1049h" not in output[cursor:], (
                            "menu returned before acknowledgement"
                        )
                    continue
                if callable(key):
                    key()
                    continue
                last_key_start = len(output)
                os.write(master, key)
                read_for(0.1)
            wait_pty(process, master, 3, receive)
            assert process.returncode == expected_status, output
            assert termios.tcgetattr(slave) == before, (
                "terminal settings were not restored"
            )
            handoff = output.find(b"SHUI_HANDOFF")
            cleanup = output[:handoff] if handoff >= 0 else output
            assert cleanup.rfind(b"\x1b[?25h") > cleanup.rfind(b"\x1b[?25l"), (
                "cursor hidden at handoff/exit"
            )
            assert cleanup.rfind(b"\x1b[?1049l") > cleanup.rfind(b"\x1b[?1049h"), (
                "alternate screen active"
            )
            assert b"\x1b[0m" in cleanup[cleanup.rfind(b"\x1b[?1049l") :], (
                "styles not reset"
            )
            if b"SHUI_EDITOR" in output:
                assert (
                    root / "command.editor-tty"
                ).read_bytes().strip() == tty_before, "editor inherited modified tty"
            if (root / "command.setup-tty").exists():
                assert (
                    root / "command.setup-tty"
                ).read_bytes().strip() == tty_before, "setup inherited modified tty"
            if handoff >= 0:
                assert (root / "command.tty").read_bytes().strip() == tty_before, (
                    "child inherited modified tty settings"
                )
            return output
        finally:
            close_pty(process, master, slave)

    # A fresh home needs no SSH directory; unset, empty and relative XDG values fall back.
    for index, xdg in enumerate((None, "", "relative-config")):
        fresh_home = root / f"fresh-home-{index}"
        fresh_home.mkdir()
        child_env["HOME"] = str(fresh_home)
        if xdg is None:
            child_env.pop("XDG_CONFIG_HOME", None)
        else:
            child_env["XDG_CONFIG_HOME"] = xdg
        session([b"\x03"])
        assert (fresh_home / ".config/shui.yml").is_file()
        assert not (fresh_home / ".ssh").exists()
    child_env["HOME"] = str(root)
    child_env["XDG_CONFIG_HOME"] = str(root / "settings")

    session(
        [b"\x1b[3~", b"ALPHA", "я".encode(), b"\x1b[3~", b"\r"]
    )  # Delete on empty filter and a Unicode character
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]
    # Enhanced text reports base and shifted codepoints. Exercise both editable
    # remote fields through Crossterm, including repeat/release and held Shift.
    shifted_remote = [
        b"\x1b[57441;2u",
        b"\x1b[117:85;2u",
        b"\x1b[117:85;2:3u",
        b"\x1b[57441;2:3u",
        b"ser",
        b"\x1b[50:64;2u",
        b"shift-input.invalid",
        b"\x1b[59:58;2u",
        b"222",
        b"\x1b[50;1:2u",
        b"\x1b[50;1:3u",
    ]
    for prefix in ([], [ADD_REMOTE]):
        output = session([*prefix, *shifted_remote, b"\r"])
        assert b"\x1b[>15u" in output, "shifted key reporting was not requested"
        assert (root / "command").read_text().splitlines()[1:] == [
            "-p",
            "2222",
            "--",
            "User@shift-input.invalid",
        ]
    session([b"beta:2222", b"\x1b[19~", b"\r"])  # F8: SSH to Mosh
    command = (root / "command").read_text().splitlines()
    assert command[0].endswith("/mosh"), command
    assert command[1:] == ["--ssh=ssh -p 2222", "--", "beta"], command
    session([b"beta:2222", b"\r"])  # Restore Mosh from the previous launch.
    assert (root / "command").read_text().splitlines()[0].endswith("/mosh")
    session([b"beta:2222", b"\x1b[D", b"\r"])  # Left from remembered Mosh to SSH
    command = (root / "command").read_text().splitlines()
    assert command[0].endswith("/ssh"), command
    assert command[1:] == ["-p", "2222", "--", "beta"], command
    session(
        [b"wrong", b"\x08", b"alphax", b"\x7f", b"\x1b[19~", b"\x1b[19~", b"\r"]
    )  # erase word, backspace, toggle twice back to SSH
    command = (root / "command").read_text().splitlines()
    assert command[0].endswith("/ssh"), command
    assert command[1:] == ["--", "alpha"], command
    for erase in (b"\x08", b"\x1b[127;5u", b"\x1b[3;5~"):
        session([erase, b"alpha extra", erase, b"\x7f", b"\x15", b"\r"])
        assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]
    session([b"\r"], args=("alpha",))
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]
    # Direct addresses avoid a setup dialog, and only redundant configured users disappear.
    output = session([b"new-direct:2202", b"\r"])
    assert b"connects to" in output
    assert (root / "command").read_text().splitlines()[1:] == [
        "-p",
        "2202",
        "--",
        "new-direct",
    ]
    session([b"tester@alpha", b"\r"])
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]
    (root / ".ssh/config").write_text("Host alpha beta\n User other\n")
    session([b"tester@alpha", b"\r"])
    assert (root / "command").read_text().splitlines()[1:] == ["--", "tester@alpha"]
    (root / ".ssh/config").write_text("Host alpha beta\n User tester\n")
    session([b"new-settings", b"\x1b[13;5u", b"\r"])
    assert (root / "command").read_text().splitlines()[1:] == ["--", "new-settings"]
    (root / ".ssh/config").write_text(
        'Match exec "false"\n User other\nHost alpha beta\n User tester\n'
    )
    session([b"tester@alpha", b"\r"])
    assert (root / "command").read_text().splitlines()[1:] == ["--", "tester@alpha"]
    (root / ".ssh/config").write_text("Host alpha beta\n User tester\n")
    # A missing final newline must not merge our entry into a previous command.
    (root / ".bash_history").write_text("echo previous")
    session([b"alpha", b"\r"])
    assert (root / ".bash_history").read_text().splitlines()[0] == "echo previous"
    # An unusable history file must not block an otherwise valid connection.
    (root / ".bash_history").unlink()
    (root / ".bash_history").mkdir()
    output = session([b"alpha", b"\r"])
    assert b"could not save connection history" in output
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]
    (root / ".bash_history").rmdir()
    (root / ".bash_history").write_text("ssh alpha\nssh -p 2222 beta\n")
    session([b"wrong-filter", b"\x15", b"alpha", b"\r"])
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]

    # Refresh retains both selected identity and profile despite changed history order.
    def changed_hosts():
        with (root / ".ssh/config").open("a") as file:
            file.write("Host alphabeta\n User other\n")
        with (root / ".bash_history").open("a") as file:
            file.write("ssh alphabeta\n")

    output = session([b"alpha", b"\x1b[C", changed_hosts, b"\x1b[15~", b"\r"])
    assert b"Refreshed" in output
    command = (root / "command").read_text().splitlines()
    assert command[0].endswith("/mosh") and command[-1] == "alpha", command
    (root / ".ssh/config").write_text("Host alpha beta\n User tester\n")
    (root / ".bash_history").write_text("ssh alpha\nssh -p 2222 beta\n")
    config_file = root / "settings/shui.yml"
    original_config = config_file.read_text()
    session([b"beta:2222", b"\x1b[C", b"\x1b[C", b"\r"])
    command = (root / "command").read_text().splitlines()
    assert command[0].endswith("/mc"), command
    assert command[1:] == ["/sh://beta:2222/"], command
    session([b"beta:2222", b"\x1b[C", b"\r"])  # Right from remembered MC to Bash
    command = (root / "command").read_text().splitlines()
    assert command[1:6] == ["-p", "2222", "-tt", "--", "beta"]
    assert "exec bash -l" in "\n".join(command[6:])
    assert config_file.read_text() == original_config, "existing config was overwritten"
    session([b"beta:2222", b"\x1b[C", b"\r"])  # Bash -> Zellij
    command = (root / "command").read_text().splitlines()
    assert command[1:6] == ["-p", "2222", "-tt", "--", "beta"], command
    # SSH receives just the remote application command, without bootstrap code.
    remote_command = "\n".join(command[6:])
    assert "attach --create" in remote_command
    # Execute the actual encoded remote command with an existing mock Zellij.
    zellij = root / "bin/zellij"
    zellij.write_text(
        '#!/bin/sh\nprintf "%s\\n" "$@" > "$SHUI_TEST_LOG.zellij"\nread -r line; [ "$line" = terminal-input ]\n'
    )
    zellij.chmod(0o755)
    remote = subprocess.run(
        ["sh", "-c", remote_command],
        env=child_env,
        input="terminal-input\n",
        text=True,
        capture_output=True,
        timeout=10,
    )
    assert remote.returncode == 0, remote.stderr
    assert (root / "command.zellij").read_text().splitlines() == [
        "attach",
        "--create",
        "options",
        "--show-startup-tips",
        "false",
        "--show-release-notes",
        "false",
    ]
    session(
        [b"beta:2222", b"\r"]
    )  # Zellij is restored from the single-line history entry.
    assert "\n".join((root / "command").read_text().splitlines()[6:]) == remote_command
    # New direct shortcuts and input-bearing forwarding profiles use normal handoff.
    session([b"beta:2222", b"\x1bt"])
    command = (root / "command").read_text().splitlines()
    assert command[1:6] == ["-p", "2222", "-tt", "--", "beta"]
    assert "tmux attach-session || exec tmux new-session" in "\n".join(command[6:])
    session([b"beta:2222", b"\x1bf"])
    assert (root / "command").read_text().splitlines()[1:] == [
        "-P",
        "2222",
        "--",
        "beta",
    ]
    session([b"beta:2222", b"\x1bh"])
    assert (root / "command").read_text().splitlines()[6:] == ["exec btop"]
    for key, flag, value in [
        (b"\x1b[108;7u", "-L", "127.0.0.1:8080:localhost:80"),
        (b"\x1b[100;7u", "-D", "127.0.0.1:1080"),
    ]:
        session([b"beta:2222", key, b"\r"])
        args = (root / "command").read_text().splitlines()[1:]
        assert args[:4] == ["-p", "2222", "-N", "-T"], args
        assert args[-4:] == [flag, value, "--", "beta"], args
    config_file.write_text("""default: Script
profiles:
  - name: Script
    command: port=%p; exec ssh ${port:+-p "$port"} -tt -- %h %{remote}
    remote: exec bash -lc '~/bin/work.sh; exec bash -l'
  - name: Custom
    command: custom-connect %h %p %{value}
    value: literal ; $HOME
""")
    session([b"beta:2222", b"\r"])
    assert (root / "command").read_text().splitlines()[1:] == [
        "-p",
        "2222",
        "-tt",
        "--",
        "beta",
        "exec bash -lc '~/bin/work.sh; exec bash -l'",
    ]
    session([b"beta:2222", b"\x1b[C", b"\r"])
    command = (root / "command").read_text().splitlines()
    assert command[0].endswith("/custom-connect"), command
    assert command[1:] == ["beta", "2222", "literal ; $HOME"], command
    session([b"beta:2222", b"\r"])  # Custom argv is restored too.
    assert (root / "command").read_text().splitlines()[0].endswith("/custom-connect")
    config_file.write_text("profiles: [")
    invalid = startup_failure()
    assert invalid.returncode != 0
    assert config_file.name.encode() in invalid.stderr
    assert b"\x1b[?1049h" not in invalid.stdout, "invalid config opened the TUI"
    config_file.write_text(original_config)
    (root / "command").unlink()
    session([b"no such host", b"\r", b"\x03"])  # invalid address, then Ctrl-C
    assert not (root / "command").exists()
    session([b"\x03"], rows=10, cols=40)
    # Profile changes belong to target identity, not the current row or filter.
    baseline = "ssh alpha\nssh -p 2222 beta\n"
    (root / ".bash_history").write_text(baseline)
    session([b"\x1b[C", b"\x1b[B", b"\r"])  # Change beta, launch unchanged alpha.
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]
    assert (root / "command").read_text().splitlines()[0].endswith("/ssh")
    (root / ".bash_history").write_text(baseline)
    session([b"\x1b[C", b"\x1b[B", b"\x1b[A", b"\r"])  # beta remembers its edit.
    assert (root / "command").read_text().splitlines()[0].endswith("/mosh")
    (root / ".bash_history").write_text(baseline)
    session([b"\x1b[C", b"alpha", b"\x08", b"\r"])  # Filtering preserves beta's edit.
    assert (root / "command").read_text().splitlines()[0].endswith("/mosh")
    (root / ".bash_history").write_text(baseline)
    session([b"\x1b[C", b"\x03"])  # Merely cycling does not save settings.
    assert (root / ".bash_history").read_text() == baseline
    assert config_file.read_text() == original_config
    session([b"beta:2222", b"\r"])
    assert (root / "command").read_text().splitlines()[0].endswith("/ssh")
    # Exercise the shipped key-auth and dotfiles recipes with fake tools.
    for name in ("ssh-copy-id", "scp"):
        stub = root / "bin" / name
        stub.write_text(
            '#!/bin/sh\nprintf "%s\\n" "$@" > "$SHUI_TEST_LOG.' + name + '"\n'
        )
        stub.chmod(0o755)
    ssh = root / "bin/ssh"
    original_ssh = ssh.read_text()
    ssh.write_text(original_ssh + 'printf "%s\\n" "$@" >> "$SHUI_TEST_LOG.calls"\n')
    session([ADD_REMOTE, b"setup@example.org:2200", b"\t", b" ", b"\r"])
    key_calls = (root / "command.calls").read_text()
    assert "authorized_keys" in key_calls and "fingerprint=" in key_calls
    assert str(root / ".ssh/id_ed25519") in key_calls.splitlines()
    (root / "src/dotfiles").mkdir(parents=True)
    (root / "src/dotfiles/install.sh").write_text("exit 0\n")
    (root / "command.calls").unlink()
    session([ADD_REMOTE, b"dotfiles@example.org:2201", b"\t", b"\t", b" ", b"\r"])
    assert not (root / "command.ssh-copy-id").exists()
    assert (root / "command.scp").read_text().splitlines() == [
        "-P",
        "2201",
        "-r",
        str(root / "src/dotfiles") + "/.",
        "dotfiles@example.org:src/dotfiles/",
    ]
    dotfiles_calls = (root / "command.calls").read_text()
    assert "-i" not in dotfiles_calls.splitlines()
    ssh.write_text(original_ssh)
    # Adding a remote persists through ordinary history, even if no history existed.
    (root / ".bash_history").unlink()
    session([ADD_REMOTE, b"new@example.org:2200", b"\r"])
    assert (root / "command").read_text().splitlines()[1:] == [
        "-p",
        "2200",
        "--",
        "new@example.org",
    ]
    assert "new@example.org:2200" in subprocess.check_output(
        [binary, "--dump"], env=child_env, text=True
    )
    before = (root / ".bash_history").read_text()
    session([ADD_REMOTE, b"invalid:0", b"\r", b"\x1b", b"\x03"])
    assert (root / ".bash_history").read_text() == before
    # Configured setup receives escaped target/port and a restored terminal before connect.
    stub = root / "bin/setup-test"
    # Force setup to outlast the inter-key delay, on success and failure.
    stub.write_text(
        '#!/bin/sh\nprintf "%s\\n" "$@" > "$SHUI_TEST_LOG.setup"\nstty -g > "$SHUI_TEST_LOG.setup-tty"\nsleep 0.3\nexit "${SETUP_EXIT:-0}"\n'
    )
    stub.chmod(0o755)
    config_file.write_text("""default: SSH
profiles: [{name: SSH, command: 'ssh -- %h'}]
configuration_scripts:
  - name: Test setup
    command: setup-test %h %p %{value}
    value: literal ; $HOME
""")
    session([ADD_REMOTE, b"added:2222", b"\t", b" ", b"\r"])
    assert (root / "command.setup").read_text().splitlines() == [
        "added",
        "2222",
        "literal ; $HOME",
    ]
    before = (root / ".bash_history").read_text()
    (root / "command").unlink()
    child_env["SETUP_EXIT"] = "7"
    session([ADD_REMOTE, b"failed", b"\t", b" ", b"\r"], expected_status=1)
    assert not (root / "command").exists()
    assert (root / ".bash_history").read_text() == before
    del child_env["SETUP_EXIT"]
    # Configure the selected remote: prefilled target, setup only, return to menu.
    before = (root / ".bash_history").read_text()
    config_before = (root / ".ssh/config").read_text()
    (root / "command").unlink(missing_ok=True)
    output = session(
        [
            b"added:2222",
            b"\x1bOS",
            b" ",
            b"\r",
            (b"Press Enter to continue...",),
            b"\r",
            (b"\x1b[?1049h",),
            b"\x03",
        ],
        acknowledge_recipes=False,
    )
    assert b"Configure host" in output and b"completed" in output
    assert b"Test setup finished (" in output
    assert (root / "command.setup").read_text().splitlines() == [
        "added",
        "2222",
        "literal ; $HOME",
    ]
    assert not (root / "command").exists(), "configure-only launched a connection"
    assert (root / ".bash_history").read_text() == before
    assert (root / ".ssh/config").read_text() == config_before
    child_env["SETUP_EXIT"] = "7"
    output = session(
        [
            b"added:2222",
            b"\x1bOS",
            b" ",
            b"\r",
            (b"Press Enter to continue...",),
            b"\r",
            (b"\x1b[?1049h",),
            b"\x03",
        ],
        acknowledge_recipes=False,
    )
    assert b"failed" in output and not (root / "command").exists()
    assert (root / ".bash_history").read_text() == before
    del child_env["SETUP_EXIT"]
    # No recipe means no side effects; cancelling and changing modes preserve drafts.
    (root / "command.setup").unlink()
    output = session([b"added:2222", b"\x1bOS", b"\r", b"\x1b", b"\x03"])
    assert b"Choose a setup recipe" in output
    assert not (root / "command.setup").exists()
    session(
        [
            b"added:2222",
            b"\x1bOS",
            b"\x1b[Z",
            b"\x1b[Z",
            b"\x1b[C",
            b"\t",
            b"draft",
            b"\x1b[Z",
            b"\x1b[D",
            b"\t",
            b"\t",
            b" ",
            b"\r",
            RETURNED,
            b"\x03",
        ]
    )
    assert (root / "command.setup").read_text().splitlines()[:2] == ["added", "2222"]
    # F4 carries a typed address into Add new; Shift+F4 always starts blank.
    session([b"no-such-host", b"\x1bOS", b"\r"])
    assert (root / "command").read_text().splitlines()[1:] == ["--", "no-such-host"]
    session([b"unused-draft", ADD_REMOTE, b"no-selection", b"\r"])
    assert (root / "command").read_text().splitlines()[1:] == ["--", "no-selection"]
    # Script-specific fields, required validation, choices, cancellation and literal input.
    config_file.write_text("""default: SSH
profiles: [{name: SSH, command: 'ssh -- %h'}]
configuration_scripts:
  - name: Inputs
    command: setup-test %h %{input:value} %{input:mode}
    inputs:
      - id: value
        type: promptString
        description: Install argument
        required: true
      - id: mode
        type: pickString
        options: [minimal, full]
  - name: Other
    command: setup-test %h
""")
    # Configure mode uses recipe inputs, skipping even a default profile's required fields.
    setup_config = config_file.read_text()
    config_file.write_text(
        setup_config.replace(
            "profiles: [{name: SSH, command: 'ssh -- %h'}]",
            """profiles:
  - name: SSH
    command: ssh -- %h %{input:launch}
    inputs: [{id: launch, type: promptString, required: true}]""",
        )
    )
    (root / "command").unlink()
    output = session(
        [
            b"added:2222",
            b"\x1bOS",
            b" ",
            b"\r",
            b"configure-value",
            b"\t",
            b"\x1b[C",
            b"\x1b\r",
            RETURNED,
            b"\x03",
        ],
        rows=9,
        cols=90,
    )
    assert b"is required" in output and b"completed" in output
    assert (root / "command.setup").read_text().splitlines() == [
        "added",
        "configure-value",
        "full",
    ]
    assert not (root / "command").exists()
    config_file.write_text(setup_config)
    literal = "a ' ; $(touch SHOULD_NOT_EXIST) %{input:mode}"
    output = session(
        [
            ADD_REMOTE,
            b"inputs-host",
            b"\t",
            b" ",
            b"\r",
            literal.encode(),
            b"\x1bOP",
            b"\x1b[6~",
            b"\x1bOP",
            b"\t",
            b"\x1b[C",
            b"\x1b[Z",
            b"\x1b[Z",
            b" ",
            b" ",
            b"\r",
        ],
        rows=9,
        cols=90,
    )
    assert b"is required" in output
    assert (root / "command.setup").read_text().splitlines() == [
        "inputs-host",
        literal,
        "full",
    ]
    assert literal not in (root / ".bash_history").read_text(), (
        "setup values must not enter history"
    )
    before = (root / ".bash_history").read_text()
    session(
        [ADD_REMOTE, b"cancelled", b"\t", b" ", b"\t", b"discard me", b"\x1b", b"\x03"]
    )
    assert (root / ".bash_history").read_text() == before
    # Generators run on selection, cache results in this dialog, and pass resolved choices.
    config_file.write_text("""default: SSH
profiles: [{name: SSH, command: 'ssh -- %h'}]
configuration_scripts:
  - name: Generated
    command: setup-test %{input:text} %{input:choice}
    inputs:
      - id: text
        type: promptString
        command: |
          printf 'default: generated text'
      - id: choice
        type: pickString
        command: |
          printf 'run\\n' >> "$HOME/generator-runs"
          printf '%s' '{"options":["first","second"],"default":"second"}'
  - name: Broken
    command: setup-test should-not-run
    inputs:
      - id: choice
        type: pickString
        command: echo 'generator test failure' >&2; exit 5
""")
    session([b"\x03"])
    assert not (root / "generator-runs").exists(), "generators ran during config load"
    session([ADD_REMOTE, b"generated", b"\t", b" ", b" ", b" ", b"\r"])
    assert (root / "command.setup").read_text().splitlines() == [
        "generated text",
        "second",
    ]
    assert (root / "generator-runs").read_text().splitlines() == ["run"], (
        "generator was rerun instead of cached"
    )
    (root / "command.setup").unlink()
    output = session(
        [ADD_REMOTE, b"broken", b"\t", b"\t", b" ", b"\r", b"\x1b", b"\x03"]
    )
    assert b"generator test failure" in output
    assert not (root / "command.setup").exists(), (
        "failed generator allowed setup to run"
    )
    # Availability filters choices, preserves original script identity, and skips generators.
    config_file.write_text("""default: SSH
profiles: [{name: SSH, command: 'ssh -- %h'}]
configuration_scripts:
  - name: Hidden
    available_command: |
      echo checked >> "$HOME/availability-checks"
      exit 1
    command: setup-test hidden
    inputs:
      - id: unused
        type: promptString
        command: touch "$HOME/hidden-generator"; printf '{}'
  - name: Visible
    available_command: exit 0
    command: setup-test visible
""")
    session([b"\x03"])
    assert not (root / "availability-checks").exists()
    output = session([ADD_REMOTE, b"available", b"\t", b" ", b"\r"])
    assert b"Running Visible for available" in output
    assert (root / "command.setup").read_text().splitlines() == ["visible"]
    assert (root / "availability-checks").read_text().splitlines() == ["checked"]
    assert not (root / "hidden-generator").exists()
    config_file.write_text(original_config)
    (root / ".ssh/id_ed25519").unlink()  # Public files alone do not make a keypair.
    output = session([ADD_REMOTE, b"no-keys", b"\t", b" ", b" ", b"\r"])
    assert not (root / ".ssh/id_ed25519").exists(), (
        "opening setup must not generate keys"
    )
    assert (root / "command").read_text().splitlines()[1:] == ["--", "no-keys"]
    (root / ".ssh/id_ed25519").write_text("fake private key")
    output = session([ADD_REMOTE, b"\t", b" ", b"\x1b", b"\x03"])
    assert (root / ".ssh/id_ed25519").read_text() == "fake private key"
    config_file.write_text(original_config)
    # Help is embedded and returns to the same filter/profile without launching anything.
    before = (root / ".bash_history").read_text()
    output = session(
        [
            b"\x1bOP",
            b"\x1b[F",
            b"\x1b[H",
            b"\t",
            b"\x1b[6~",
            b"\x1b[5~",
            b"\x1b[Z",
            b"\x1bOP",
            b"\x03",
        ],
        rows=10,
        cols=40,
    )
    assert b"HELP" in output and b"Profiles" in output
    assert b"Setting up a remote" not in output, "host help offered dialog-only topics"
    assert (root / ".bash_history").read_text() == before
    (root / ".bash_history").write_text("ssh alpha\n")
    session([b"alpha", b"\x1b[C", b"\x1bOP", b"\x1b[C", b"\x1bOP", b"\r"])
    assert (root / "command").read_text().splitlines()[0].endswith("/mosh")
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]
    output = session(
        [ADD_REMOTE, b"help-host:2222", b"\x1bOP", b"\t", b"\x1b[D", b"\x1bOP", b"\r"]
    )
    assert b"Setting up a remote" in output
    assert b"Profiles" not in output and b"Keys" in output, (
        "dialog help should offer only setup and active keys"
    )
    assert (root / "command").read_text().splitlines()[1:] == [
        "-p",
        "2222",
        "--",
        "help-host",
    ]
    # Unbound Ctrl+Q must not quit the menu or close help/dialogs.
    session([b"\x11", b"alpha", b"\r"])
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]
    session([b"\x1bOP", b"\x11", b"\x1bOP", b"\x03"])
    output = session([ADD_REMOTE, b"\x11", b"ctrlq-host", b"\r"])
    assert (root / "command").read_text().splitlines()[1:] == ["--", "ctrlq-host"]
    # Preparation runs with a restored tty and must succeed before connection/history.
    prepare = root / "bin/prepare-test"
    prepare.write_text(
        '#!/bin/sh\nprintf "%s\\n" "$@" > "$SHUI_TEST_LOG.prepare"\nstty -g > "$SHUI_TEST_LOG.setup-tty"\nprintf "Install test dependency? [y/N] "\nread -r reply\n[ "$reply" = y ]\n'
    )
    prepare.chmod(0o755)
    config_file.write_text(
        "default: SSH\nprofiles: [{name: SSH, command: 'ssh -- %h', prepare_command: 'prepare-test %h %p %{literal}', literal: 'a ; $HOME'}]\n"
    )
    before = (root / ".bash_history").read_text()
    (root / "command").unlink(missing_ok=True)
    output = session(
        [
            b"help-host:2222",
            b"\r",
            (b"Install test dependency? [y/N] ",),
            b"n\n",
            (b"\x1b[?1049h",),
            b"\x03",
        ]
    )
    assert b"preparation cancelled or failed" in output
    assert (
        not (root / "command").exists()
        and (root / ".bash_history").read_text() == before
    )
    assert (root / "command.prepare").read_text().splitlines() == [
        "help-host",
        "2222",
        "a ; $HOME",
    ]
    session([b"help-host:2222", b"\r", (b"Install test dependency? [y/N] ",), b"y\n"])
    assert (root / "command").read_text().splitlines()[1:] == ["--", "help-host"]
    assert "prepare-test" not in (root / ".bash_history").read_text()
    # Launch availability is evaluated once at load, before history and navigation.
    config_file.write_text("""default: Missing
profiles:
  - name: Missing
    available_command: echo missing >> "$HOME/profile-checks"; exit 1
    command: custom-connect %h
  - name: SSH
    available_command: echo ssh >> "$HOME/profile-checks"; command -v ssh
    command: ssh -- %h
  - name: Mosh
    available_command: echo mosh >> "$HOME/profile-checks"; command-that-does-not-exist
    command: mosh -- %h
configuration_scripts:
  - name: Setup
    available_command: touch "$HOME/unexpected-setup-check"
    command: true
""")
    for flag in ("--dump", "--help", "--version"):
        subprocess.run([binary, flag], env=child_env, capture_output=True, check=True)
    assert not (root / "profile-checks").exists(), (
        "noninteractive mode ran availability checks"
    )
    (root / ".bash_history").write_text("mosh alpha\n")
    output = session(
        [b"alpha", b"\x1b[C", b"\x1b[D", b"\x1b[19~", b"\x1bOP", b"\x1bOP", b"\r"]
    )
    assert (root / "profile-checks").read_text().splitlines() == [
        "missing",
        "ssh",
        "mosh",
    ]
    assert not (root / "unexpected-setup-check").exists()
    assert b"Missing" not in output and b"command-that-does-not-exist" not in output
    assert (root / "command").read_text().splitlines()[0].endswith("/ssh")
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]
    # All unavailable is an actionable error, without entering the alternate screen.
    config_file.write_text(
        "default: Missing\nprofiles: [{name: Missing, available_command: 'exit 1', command: 'ssh %h'}]\n"
    )
    result = startup_failure()
    assert (
        result.returncode != 0 and b"no launch profiles are available" in result.stderr
    )
    assert b"\x1b[?1049h" not in result.stdout
    config_file.write_text(original_config)
    config_file.write_text("""default: SSH
profiles:
  - {name: SSH, key: Ctrl+Q, command: 'ssh -- %h'}
  - {name: Mosh, key: Alt+M, command: 'mosh -- %h'}
  - {name: Custom, key: Ctrl+Alt+X, command: 'custom-connect %h'}
""")
    (root / ".bash_history").write_text("ssh alpha\n")
    for shortcut, program, arguments in [
        (b"\x1bm", "mosh", ["--", "alpha"]),
        (b"\x11", "ssh", ["--", "alpha"]),
        (b"\x1b\x18", "custom-connect", ["alpha"]),
    ]:
        session([b"alpha", shortcut])
        command = (root / "command").read_text().splitlines()
        assert command[0].endswith("/" + program) and command[1:] == arguments, command
    # Profile cycling and shortcuts also work for valid addresses absent from history/config.
    session([b"direct-cycle", b"\x1b[C", b"\r"])
    command = (root / "command").read_text().splitlines()
    assert command[0].endswith("/mosh") and command[1:] == ["--", "direct-cycle"], (
        command
    )
    session([b"direct-shortcut", b"\x1b\x18"])
    command = (root / "command").read_text().splitlines()
    assert command[0].endswith("/custom-connect") and command[1:] == [
        "direct-shortcut"
    ], command
    # The directly launched profile is restored on the next ordinary Enter.
    session([b"alpha", b"\r"])
    assert (root / "command").read_text().splitlines()[0].endswith("/custom-connect")
    before = (root / ".bash_history").read_text()
    (root / "command").unlink()
    session([b"m", b"\x03"])  # Plain typing must not launch the Alt+M profile.
    session([b"not a valid address", b"\x1bm", b"\x03"])
    session([b"\x1bOP", b"\x1bm", b"\x1bOP", b"\x03"])  # Help does not launch profiles.
    session([ADD_REMOTE, b"\x1bm", b"\x1b", b"\x03"])  # Neither does the dialog.
    assert not (root / "command").exists()
    assert (root / ".bash_history").read_text() == before
    config_file.write_text(original_config)
    # Fuzzy abbreviations and complete user-specific SSH config hints.
    (root / ".bash_history").write_text("ssh alpha\n")
    (root / ".ssh/config").write_text(
        "  # Production note\nHost alpha # primary\n User tester\n HostName 192.0.2.10\n Port 2222\n"
    )
    output = session([b"aph", b"\r"])
    assert b"Production note" in output
    assert b"Host alpha # primary" in output
    assert b"HostName 192.0.2.10" in output
    assert b"Port 2222" in output
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]

    # Hint-only and mixed queries select the alias, including with the hint hidden.
    for query in [b"prdctn", b"192.0.2.10", b"alpha 2222", b"tester production"]:
        session([query, b"\r"], rows=10, cols=40)
        assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]

    # One-off inputs remain literal and do not enter history.
    config_file.write_text("""default: SSH
profiles:
  - {name: SSH, command: 'ssh -- %h'}
  - name: Custom
    key: Alt+X
    command: custom-connect %h %{input:argument}
    inputs:
      - {id: argument, type: promptString, required: true}
""")
    before = (root / ".bash_history").read_text()
    output = session(
        [
            b"alpha",
            b"\x1b[13;5u",
            b"\x08",
            b"other:2222",
            b"\t",
            b"\x1b[C",
            b"\r",
            b"literal; $(false)",
            b"\r",
        ]
    )
    assert b"CONNECT" in output and b"required" in output
    assert (root / "command").read_text().splitlines()[1:] == [
        "other",
        "literal; $(false)",
    ]
    assert (root / ".bash_history").read_text() == before
    session([b"alpha", b"\x1bx", b"shortcut", b"\r"])
    assert (root / "command").read_text().splitlines()[1:] == ["alpha", "shortcut"]

    # Return mode restores the child tty and then preserves the filter.
    config_file.write_text(
        "default: SSH\nprofiles: [{name: SSH, command: 'ssh -- %h'}]\n"
    )
    child_env["SHUI_TEST_DELAY"] = "0.3"  # Exceed the old fixed inter-key delay.
    output = session([b"alpha", b"\x1b\r", RETURNED, b"\x1b\r", RETURNED, b"\x03"])
    del child_env["SHUI_TEST_DELAY"]
    assert output.count(b"SHUI_HANDOFF") == 2

    # Editor handoff, valid reload, and invalid edits retaining the active config.
    editor = root / "bin/test-editor"
    editor.write_text(
        '#!/bin/sh\nstty -g > "$SHUI_TEST_LOG.editor-tty"\ncp "$HOME/edited.yml" "$1"\nprintf "SHUI_EDITOR\\n"\n'
    )
    editor.chmod(0o755)
    child_env["VISUAL"] = str(editor)
    child_env["EDITOR"] = str(root / "missing-editor")
    (root / "edited.yml").write_text(
        "default: Custom\nprofiles: [{name: Custom, command: 'custom-connect %h'}]\n"
    )
    output = session([b"alpha", b"\x1b[20~", RETURNED, b"\r"])
    assert output.count(b"SHUI_EDITOR") == 1
    assert b"CONFIGURATION" not in output
    assert (root / "command").read_text().splitlines()[0].endswith("/custom-connect")
    # Explicit EDITOR wins; empty values defer to the system editor selector.
    child_env["VISUAL"] = " "
    child_env["EDITOR"] = str(editor)
    output = session([b"\x1b[20~", RETURNED, b"\x03"])
    assert output.count(b"SHUI_EDITOR") == 1
    # Isolate PATH so these cases never open a real system editor.
    for tool in ("cp", "stty"):
        (root / "bin" / tool).symlink_to(shutil.which(tool))
    previous_path = child_env["PATH"]
    child_env["PATH"] = str(root / "bin")
    child_env["EDITOR"] = ""
    shutil.copy2(editor, root / "bin/mcedit")
    selector = root / "bin/sensible-editor"
    selector.write_text(
        '#!/bin/sh\n[ -z "${VISUAL:-}" ] && [ -z "${EDITOR:-}" ] || exit 9\nprintf "SYSTEM_EDITOR\\n"\nexec mcedit "$@"\n'
    )
    selector.chmod(0o755)
    shutil.copy2(editor, root / "bin/editor")
    shutil.copy2(editor, root / "bin/vi")
    # Never replace an explicit missing editor or an editor that ran and failed.
    child_env["VISUAL"] = str(root / "missing-editor")
    output = session([b"\x1b[20~", RETURNED, b"\x1b", b"\x03"])
    assert b"Configuration not reloaded" in output and b"SHUI_EDITOR" not in output
    child_env["VISUAL"] = " "
    selector_source = selector.read_text()
    selector.write_text("#!/bin/sh\nexit 7\n")
    output = session([b"\x1b[20~", RETURNED, b"\x1b", b"\x03"])
    assert b"Editor exited" in output and b"SHUI_EDITOR" not in output
    selector.write_text(selector_source)
    for fallback in ("sensible-editor", "editor", "vi"):
        output = session([b"\x1b[20~", RETURNED, b"\x03"])
        assert output.count(b"SHUI_EDITOR") == 1
        assert (b"SYSTEM_EDITOR" in output) == (fallback == "sensible-editor")
        (root / "bin" / fallback).unlink()
    child_env["PATH"] = previous_path
    child_env["VISUAL"] = str(editor)
    (root / "edited.yml").write_text("profiles: [broken\n")
    output = session(
        [
            b"alpha",
            b"\x1b[20~",
            RETURNED,
            b"\x1bOP",
            b"\x1bOP",
            b"\r",
            RETURNED,
            b"\x1b",
            b"\r",
        ]
    )
    assert output.count(b"SHUI_EDITOR") == 2
    assert b"Configuration not reloaded" in output
    assert (root / "command").read_text().splitlines()[0].endswith("/custom-connect")
    config_file.write_text(original_config)
    # App bindings replace defaults, remain scoped, and drive both dispatch and hints.
    config_file.write_text("""default: SSH
profiles: [{name: SSH, command: 'ssh -- %h'}]
keybindings:
  menu:
    connect: [F3]
    connect_and_return: [Ctrl+R]
    connection_settings: [F5]
    refresh: []
    help: [F6]
    new_remote: [F2]
    configure_remote: [F9]
    edit_config: [F10]
    quit: [Ctrl+Q]
  dialog:
    connect: [F3]
    connect_and_return: [F5]
    help: [F6]
    cancel: [F7]
  help:
    help: []
    close: [X]
  editor:
    help: [F6]
    edit: [F3]
    cancel: [F7]
""")
    output = session([b"alpha", b"\r", b"\x1bOR"])
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]
    assert b"F3" in output
    output = session([b"alpha", b"\x12", RETURNED, b"\x12", RETURNED, b"\x11"])
    assert output.count(b"SHUI_HANDOFF") == 2
    # Former help-close keys must no longer close help; plain X may close only there.
    session([b"alpha", b"\x1b[17~", b"\x1bOP", b"q", b"x", b"\x1bOR"])
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]
    output = session([b"alpha", b"\x1b[15~", b"\r", b"\x1b[15~", RETURNED, b"\x11"])
    assert output.count(b"SHUI_HANDOFF") == 1
    session([b"\x1bOQ", b"new-rebound", b"\x1bOR"])
    assert (root / "command").read_text().splitlines()[1:] == ["--", "new-rebound"]
    config_file.write_text(
        config_file.read_text()
        + "\nconfiguration_scripts: [{name: Rebound setup, command: 'setup-test %h'}]\n"
    )
    (root / "command").unlink()
    output = session([b"alpha", b"\x1b[20~", b" ", b"\x1bOR", RETURNED, b"\x11"])
    assert b"completed" in output and b"Apply" in output
    assert (root / "command.setup").read_text().splitlines() == ["alpha"]
    assert not (root / "command").exists()
    # Editor error actions use their own bindings and help returns to the error screen.
    session(
        [b"alpha", b"\x1b[21~", RETURNED, b"\x1b[17~", b"x", b"\x1b[18~", b"\x1bOR"]
    )
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]
    config_file.write_text("""default: SSH
profiles: [{name: SSH, key: Ctrl+C, command: 'ssh -- %h'}]
keybindings: {menu: {quit: [Esc]}}
""")
    session([b"alpha", b"\x03"])
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]
    # Return action survives an input dialog; a failed connection also returns.
    config_file.write_text("""default: Custom
profiles:
  - name: Custom
    command: custom-connect %h %{input:value}
    inputs: [{id: value, type: promptString, default: test}]
""")
    output = session([b"alpha", b"\x1b\r", b"\r", RETURNED, b"\x03"])
    assert output.count(b"SHUI_HANDOFF") == 1
    config_file.write_text(
        "default: Fail\nprofiles: [{name: Fail, command: 'exit 7'}]\n"
    )
    session([b"alpha", b"\x1b\r", RETURNED, b"\x03"])
    config_file.write_text(original_config)
    # Enhanced keyboard repeat events still edit the filter.
    session([b"alphx", b"\x1b[127;1:2u", b"a", b"\r"])
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]
    # Modifier press/release reveals the correct group without changing the filter.
    config_file.write_text("""default: SSH
profiles:
  - {name: SSH, key: Alt+S, command: 'ssh -- %h'}
  - {name: Special, key: Ctrl+Alt+X, command: 'custom-connect %h'}
""")
    output = session(
        [
            b"alpha",
            b"\x1b[57443;3u",
            b"\x1b[57442;7u",
            b"\x1b[57442;7:3u",
            b"\x1b[57443;3:3u",
            b"\r",
        ]
    )
    assert b"Special" in output
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]
    output = session([b"alpha", b"\x1b?", b"\x1b?", b"\r"])
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]
    # Shift gets its own menu; releasing it restores the ordinary F4 setup action.
    output = session(
        [b"alpha", b"\x1b[57441;2u", b"\x1b[57441;2:3u", b"\x1bOS", b"\x1b", b"\r"]
    )
    assert b"Shift" in output and b"New" in output and b"Configure host" in output
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]
    output = session([b"alpha", b"\x1b[1;2P", b"\x1b[1;2P", b"\r"])
    assert b"Shift" in output
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]
    # Revealing and releasing keys also works inside help and connection dialogs.
    session(
        [
            b"alpha",
            b"\x1bOP",
            b"\x1b[57443;3u",
            b"\x1b[57443;3:3u",
            b"\x1bOP",
            b"\x1b[13;5u",
            b"\x1b[57442;5u",
            b"\x1b[57442;5:3u",
            b"\r",
        ]
    )
    assert (root / "command").read_text().splitlines()[1:] == ["--", "alpha"]
    config_file.write_text(original_config)
    # Missing pairs offer generation but merely opening/cancelling never generates a key.
    keygen = root / "bin/ssh-keygen"
    keygen.write_text("""#!/bin/sh
set -eu
printf '%s\\n' "$@" > "$SHUI_TEST_LOG.keygen"
stty -g > "$SHUI_TEST_LOG.keygen-tty"
[ ! -f "$HOME/keygen-fail" ] || exit 1
while [ "$#" -gt 0 ]; do
  if [ "$1" = -f ]; then shift; private=$1; fi
  shift
done
printf 'generated private key' > "$private"
printf 'generated public key' > "$private.pub"
""")
    keygen.chmod(0o755)
    ssh.write_text(
        original_ssh
        + 'case "$*" in *authorized_keys*) printf "%s\\n" "$@" >> "$SHUI_TEST_LOG.key-copy" ;; esac\n'
    )
    for keyfile in (root / ".ssh").glob("id_*"):
        keyfile.unlink()
    for suffix in ("keygen", "ssh-copy-id"):
        (root / ("command." + suffix)).unlink(missing_ok=True)
    before = (root / ".bash_history").read_text()
    output = session([ADD_REMOTE, b"\t", b" ", b"\x1b", b"\x03"])
    assert not (root / "command.keygen").exists()
    assert not (root / ".ssh/id_ed25519").exists()
    # Never overwrite a partial default pair, and don't copy anything after failure.
    (root / ".ssh/id_ed25519.pub").write_text("existing public key")
    session([ADD_REMOTE, b"blocked", b"\t", b" ", b"\r"], expected_status=1)
    assert (root / ".ssh/id_ed25519.pub").read_text() == "existing public key"
    assert not (root / "command.keygen").exists()
    assert not (root / "command.ssh-copy-id").exists()
    assert not (root / "command.key-copy").exists()
    assert (root / ".bash_history").read_text() == before
    (root / ".ssh/id_ed25519.pub").unlink()
    (root / "keygen-fail").touch()
    session([ADD_REMOTE, b"failed", b"\t", b" ", b"\r"], expected_status=1)
    assert not (root / "command.ssh-copy-id").exists()
    assert (root / ".bash_history").read_text() == before
    (root / "keygen-fail").unlink()
    # Key-auth generates before copying, with prompts on the restored tty.
    session([ADD_REMOTE, b"generated:2202", b"\t", b" ", b"\r"])
    assert (root / "command.keygen").read_text().splitlines() == [
        "-q",
        "-t",
        "ed25519",
        "-f",
        str(root / ".ssh/id_ed25519"),
    ]
    copied = (root / "command.key-copy").read_text()
    assert "fingerprint=" in copied and "generated" in copied.splitlines()
    assert str(root / ".ssh/id_ed25519") in copied.splitlines()
    assert (root / "command.keygen-tty").read_bytes() == (
        root / "command.tty"
    ).read_bytes()
    # Dotfiles installation uses normal SSH auth and does not generate/copy a key.
    for name in ("id_ed25519", "id_ed25519.pub"):
        (root / ".ssh" / name).unlink(missing_ok=True)
    for suffix in ("keygen", "ssh-copy-id", "scp"):
        (root / ("command." + suffix)).unlink(missing_ok=True)
    session([ADD_REMOTE, b"generated-dotfiles:2202", b"\t", b"\t", b" ", b"\r"])
    assert not (root / "command.keygen").exists()
    assert not (root / "command.ssh-copy-id").exists()
    assert (root / "command.scp").read_text().splitlines() == [
        "-P",
        "2202",
        "-r",
        str(root / "src/dotfiles") + "/.",
        "generated-dotfiles:src/dotfiles/",
    ]
    # Queued recipes run in configuration order; a failure prevents later work
    # and the connection, even when recipes were checked in reverse order.
    config_file.write_text("""default: SSH
profiles: [{name: SSH, command: 'ssh -- %h'}]
configuration_scripts:
  - name: First queued
    command: 'echo first >> "$HOME/queue-log"; test ! -f "$HOME/queue-fail"'
  - name: Second queued
    command: 'echo second >> "$HOME/queue-log"'
""")
    queue_keys = [
        ADD_REMOTE,
        b"queued:2202",
        b"\t",
        b"\t",
        b" ",
        b"\x1b[A",
        b" ",
        b"\r",
    ]
    output = session(queue_keys)
    assert output.count(b"Press Enter to continue...") == 2
    assert (root / "queue-log").read_text().splitlines() == ["first", "second"]
    (root / "queue-log").unlink()
    (root / "queue-fail").touch()
    (root / "command").unlink()
    output = session(queue_keys, expected_status=1)
    assert output.count(b"Press Enter to continue...") == 1
    assert (root / "queue-log").read_text().splitlines() == ["first"]
    assert not (root / "command").exists()
    # A real controlling TTY must remain inaccessible to background checks.
    config_file.write_text("""default: SSH
profiles:
  - name: SSH
    available_command: if (printf probe > /dev/tty) 2>/dev/null; then touch "$HOME/tty-leak"; fi; true
    command: ssh -- %h
""")
    print("Checking controlling-terminal isolation", flush=True)
    faulthandler.dump_traceback_later(30, repeat=True)
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))
    # A fresh interpreter acquires the controlling terminal after setsid.
    # Avoid Python work after pty.fork(): on macOS it can be unsafe when
    # higher-level system APIs have been used in the parent process.
    launcher = """
import fcntl, os, sys, termios
fcntl.ioctl(0, termios.TIOCSCTTY, 0)
with open('/dev/tty', 'wb', buffering=0):
    pass
os.execv(sys.argv[1], [sys.argv[1]])
"""
    process = subprocess.Popen(
        [sys.executable, "-c", launcher, str(binary)],
        stdin=slave,
        stdout=slave,
        stderr=slave,
        env=child_env,
        start_new_session=True,
    )
    try:
        output = bytearray()
        deadline = time.monotonic() + 5
        while b"SHUI" not in output and time.monotonic() < deadline:
            if select.select([master], [], [], 0.05)[0]:
                chunk = read_pty(master)
                if not chunk:
                    break
                output.extend(chunk)
        assert b"SHUI" in output, output
        assert not (root / "tty-leak").exists(), (
            "background check accessed the controlling terminal"
        )
        os.write(master, b"\x03")
        assert wait_pty(process, master, 3, output.extend) == 0, (
            "TTY probe did not exit successfully"
        )
    finally:
        close_pty(process, master, slave)
    faulthandler.cancel_dump_traceback_later()
    print(
        "PASS: discovery, editing, per-entry profiles and history restore, SSH/Mosh/MC/script/custom handoff, config validation, adding/configuring remotes and setup recipes, embedded help, terminal restoration"
    )

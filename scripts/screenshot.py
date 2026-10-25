"""Capture the actual TUI with demo hosts; requires Pillow and pyte."""

import argparse
import fcntl
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import tempfile
import termios
import time

from PIL import Image, ImageDraw, ImageFont
import pyte

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument(
    "--binary",
    type=Path,
    help="Shui executable (default: target/x86_64-unknown-linux-musl/release/shui)",
)
modes = parser.add_mutually_exclusive_group()
modes.add_argument(
    "--search", action="store_true", help="capture a search matching config comments"
)
modes.add_argument(
    "--monitor",
    action="store_true",
    help="capture the real btop dashboard (requires local btop)",
)
modes.add_argument(
    "--shift-shortcuts", action="store_true", help="capture the Shift shortcut bar"
)
modes.add_argument(
    "--shortcuts", action="store_true", help="capture the Alt shortcut bar"
)
modes.add_argument(
    "--connection", action="store_true", help="capture the connection dialog"
)
modes.add_argument(
    "--configure", action="store_true", help="capture setup for the selected host"
)
modes.add_argument(
    "--dialog", action="store_true", help="capture the new-remote dialog"
)
modes.add_argument("--help-viewer", action="store_true", help="capture built-in help")
args = parser.parse_args()
if args.search:
    keys = b"maintenance"
    title, output_name, quit_keys = "SHUI", "search.png", b"\x03"
elif args.monitor:
    keys = b"\x1bh"
    title, output_name, quit_keys = "cpu", "monitor.png", b"q"
elif args.shift_shortcuts:
    keys = b"\x1b[57441;2u"
    title, output_name, quit_keys = (
        "SHUI",
        "shift-shortcuts.png",
        b"\x1b[57441;2:3u\x03",
    )
elif args.shortcuts:
    keys = b"\x1b[57443;3u"
    title, output_name, quit_keys = "SHUI", "shortcuts.png", b"\x1b[57443;3:3u\x03"
elif args.connection:
    keys = b"\x1b[13;5u"
    title, output_name, quit_keys = "CONNECT", "connection.png", b"\x03\x03"
elif args.configure:
    keys = b"\x1bOS\x1b[C\t"
    title, output_name, quit_keys = "REMOTE SETUP", "configure-remote.png", b"\x03\x03"
elif args.dialog:
    keys = b"\x1b[1;2Sdeploy@new.example.org:2222\t\x1b[C\x1b[C\t"
    title, output_name, quit_keys = "REMOTE SETUP", "new-remote.png", b"\x03\x03"
elif args.help_viewer:
    keys = b"\x1bOP\x1b[C"
    title, output_name, quit_keys = "HELP", "help.png", b"\x03\x03"
else:
    keys = b"\x1b[C" * 4
    title, output_name, quit_keys = "SHUI", "hosts.png", b"\x03"

repo = Path(__file__).resolve().parents[1]
binary = (
    args.binary or repo / "target/x86_64-unknown-linux-musl/release/shui"
).resolve()
if not binary.is_file() or not os.access(binary, os.X_OK):
    parser.error(f"not an executable file: {binary}")
cols, rows = (110, 30) if args.monitor else (90, 22)
screen = pyte.Screen(cols, rows)
stream = pyte.ByteStream(screen)
with tempfile.TemporaryDirectory(prefix="shui-screenshot-") as directory:
    home = Path(directory)
    (home / "bin").mkdir()
    for program in ("ssh", "mosh", "mc", "sftp"):
        stub = home / "bin" / program
        stub.write_text("#!/bin/sh\nexit 0\n")
        stub.chmod(0o755)
    if args.monitor:
        # Run the profile's real remote script locally; never contact a server.
        (home / "bin/ssh").write_text(
            '#!/bin/sh\nwhile [ "$#" -gt 1 ]; do shift; done\nexec sh -c "$1"\n'
        )
    (home / ".ssh").mkdir()
    (home / ".config").mkdir(parents=True)
    (home / ".ssh/config").write_text(
        "# Production · deploy window 02:00 UTC\nHost production.example.org\n User deploy\n HostName 192.0.2.10\n Port 2222\n # Check backups before maintenance\n"
    )
    (home / ".ssh/id_ed25519").write_text("demo fixture")
    (home / ".ssh/id_ed25519.pub").write_text("demo fixture")
    (home / ".bash_history").write_text(
        "ssh deploy@staging.example.org\n"
        "ssh -p 2222 admin@backup.example.org\n"
        "mosh dev@workstation.example.org\n"
        "ssh ops@monitor.example.org\n"
        "mosh dev@build.example.org\n"
        "ssh production.example.org\n"
    )
    if args.connection:
        (home / ".config/shui.yml").write_text("""default: Remote command
profiles:
  - name: Remote command
    command: ssh %h %{input:remote}
    inputs:
      - id: remote
        type: promptString
        description: Remote command
        default: journalctl -f -u app
        required: true
""")
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    child_env = dict(
        os.environ,
        HOME=str(home),
        XDG_CONFIG_HOME=str(home / ".config"),
        USER="demo",
        TERM="xterm-256color",
        PATH=str(home / "bin") + os.pathsep + os.environ["PATH"],
    )
    child_env.pop("NO_COLOR", None)
    child = subprocess.Popen(
        [str(binary)], stdin=slave, stdout=slave, stderr=slave, env=child_env
    )
    try:
        keys_sent = False
        deadline = time.monotonic() + (4 if args.monitor else 1)
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.05)[0]:
                data = os.read(master, 65536)
                stream.feed(data)
                if not keys_sent and any("SHUI" in line for line in screen.display):
                    os.write(master, keys)
                    keys_sent = True
        assert any(title in line for line in screen.display), screen.display
        os.write(master, quit_keys)
        child.wait(timeout=3)
        assert child.returncode == 0
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
        os.close(master)
        os.close(slave)

palette = {
    "black": "#1e1e2e",
    "red": "#e06c75",
    "green": "#98c379",
    "brown": "#e5c07b",
    "blue": "#61afef",
    "magenta": "#c678dd",
    "cyan": "#56b6c2",
    "white": "#abb2bf",
    "brightblack": "#5c6370",
    "brightred": "#ff7a85",
    "brightgreen": "#b3f58c",
    "brightbrown": "#ffe38a",
    "brightblue": "#82baff",
    "brightmagenta": "#e6a0ff",
    "brightcyan": "#79edff",
    "brightwhite": "#ffffff",
}


def color(value, default):
    if value == "default":
        return default
    return palette.get(value, "#" + value)


font_path = os.environ.get("SHUI_SCREENSHOT_FONT") or subprocess.check_output(
    ["fc-match", "-f", "%{file}", "DejaVu Sans Mono for Powerline"], text=True
)
font = ImageFont.truetype(font_path, 18)
cell_w, cell_h, pad = 11, 24, 18
image = Image.new("RGB", (cols * cell_w + 2 * pad, rows * cell_h + 2 * pad), "#1e1e2e")
draw = ImageDraw.Draw(image)
for row in range(rows):
    for col in range(cols):
        char = screen.buffer[row][col]
        fg, bg = color(char.fg, "#cdd6f4"), color(char.bg, "#1e1e2e")
        if char.reverse:
            fg, bg = bg, fg
        x, y = pad + col * cell_w, pad + row * cell_h
        draw.rectangle((x, y, x + cell_w - 1, y + cell_h - 1), fill=bg)
        draw.text((x, y), char.data, font=font, fill=fg, stroke_width=0)
output_path = repo / "docs/images" / output_name
output_path.parent.mkdir(parents=True, exist_ok=True)
image.save(output_path)
print(f"Updated {output_path.relative_to(repo)}")

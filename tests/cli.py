"""Check noninteractive CLI behavior without a terminal or real SSH connections."""
import os
import json
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile

binary = str(Path(sys.argv[1]).resolve())
wine = os.environ.get("SHUI_TEST_WINE")
windows = os.name == "nt" or bool(wine)

def target_path(path):
    path = Path(path).resolve()
    return "Z:" + str(path).replace("/", "\\") if wine else str(path)

runner = [wine, os.environ["SHUI_TEST_WINE_RUNNER"]] if wine else []
if wine:
    binary = target_path(binary)
with tempfile.TemporaryDirectory(prefix="shui cli ü-") as directory:
    root = Path(directory)
    env = {key: value for key, value in os.environ.items()
           if key not in ("HOME", "USER", "USERPROFILE", "USERNAME", "XDG_CONFIG_HOME", "DISPLAY", "WAYLAND_DISPLAY")}

    def run(*args):
        if wine:
            env["SHUI_TEST_USERPROFILE"] = env.get("USERPROFILE", "")
            env["SHUI_TEST_USERNAME"] = env.get("USERNAME", "")
        return subprocess.run([*runner, binary, *args], env=env, capture_output=True,
                              text=True, encoding="utf-8", timeout=15 if wine else 5)

    for option in ("--help", "--version"):
        result = run(option)
        assert result.returncode == 0, result.stderr
        assert "shui" in result.stdout.lower()
    assert run("--unknown").returncode != 0

    defaults = run("--default-config")
    assert defaults.returncode == 0, defaults.stderr
    assert "Optional examples:" in defaults.stdout
    assert ("putty.exe" in defaults.stdout) == windows
    defaults_file = root / "defaults.yml"
    defaults_file.write_text(defaults.stdout, encoding="utf-8")
    assert run("--check-config", target_path(defaults_file)).returncode == 0
    assert not (root / "settings").exists()

    marker = root / "executed"
    config = root / "custom.yml"
    config.write_text(f"""default: SSH
profiles:
  - name: SSH
    available_command: touch '{marker}'
    prepare_command: touch '{marker}'
    command: ssh %h
""", encoding="utf-8")
    result = run("--check-config", target_path(config))
    assert result.returncode == 0, result.stderr
    assert "configuration is valid" in result.stdout
    assert not marker.exists(), "validation executed trusted shell code"
    valid_source = config.read_text(encoding="utf-8")
    config.write_text("profiles: [", encoding="utf-8")
    assert run("--check-config", target_path(config)).returncode != 0
    assert run("--check-config", target_path(root / "missing.yml")).returncode != 0

    env["USERPROFILE" if windows else "HOME"] = target_path(root)
    env["USERNAME" if windows else "USER"] = "local-user"
    if windows:
        env["HOME"] = "/wrong-posix-home"  # Native Windows must ignore Git Bash HOME.
    env["XDG_CONFIG_HOME"] = target_path(root / "settings")
    assert run("--config-path").stdout.strip() == target_path(root / "settings/shui.yml")
    assert not (root / "settings").exists(), "read-only CLI created config"
    assert run("--check-config").returncode != 0
    assert not (root / "settings").exists(), "validation created a missing config"
    result = run()
    assert result.returncode != 0 and "requires a terminal" in result.stderr
    assert "\x1b" not in result.stdout
    assert not (root / "settings").exists(), "failed TUI startup created config"

    (root / ".ssh/conf.d").mkdir(parents=True)
    (root / ".ssh/config").write_text('Include conf.d/*\nHost="local"\n User="deploy"\n', encoding="utf-8")
    (root / ".ssh/conf.d/servers").write_text("Host included\n User other\n", encoding="utf-8")
    result = run("--dump")
    assert result.returncode == 0, result.stderr
    assert set(result.stdout.splitlines()) == {"local", "included"}, result.stdout
    assert not marker.exists()
    assert not (root / "settings").exists()
    default_config = root / "settings/shui.yml"
    default_config.parent.mkdir(parents=True)
    default_config.write_text(valid_source, encoding="utf-8")
    assert run("--check-config").returncode == 0
    assert not marker.exists(), "default config validation executed a command"
    assert default_config.read_text(encoding="utf-8") == valid_source
    default_config.write_text(
        valid_source
        + """  - name: Work
    shell: direct
    command: custom-client --target %h
    port_args: ['--port', '%p']
""",
        encoding="utf-8",
    )
    (root / ".bash_history").write_text(
        "custom-client --port 2222 --target 'alice@[::1]'\n"
        "custom-client --target only-in-history\n"
        "custom-client --port 65536 --target invalid\n",
        encoding="utf-8",
    )
    result = run("--dump")
    assert set(result.stdout.splitlines()) == {
        "local",
        "included",
        "alice@[::1]:2222",
        "only-in-history",
    }, result
    assert not marker.exists(), "history discovery ran a configured command"
    default_config.write_text(valid_source, encoding="utf-8")

    env["XDG_CONFIG_HOME"] = "relative/settings"
    assert run("--config-path").stdout.strip() == target_path(root / ".config/shui.yml")
    env["XDG_CONFIG_HOME"] = target_path(root / "settings")

    if sys.platform.startswith("linux") and not wine:
        import pty

        # Isolate PATH: these tests must never open a real desktop terminal.
        launchers = root / "launchers"
        launchers.mkdir()
        (launchers / "sh").symlink_to("/bin/sh")
        log = root / "terminal.jsonl"
        server_dir = root / "terminal-server"
        server_dir.mkdir()
        env.update(PATH=str(launchers), SHUI_TEST_TERMINAL_LOG=str(log),
                   SHUI_TEST_TERMINAL_CWD=str(server_dir), TERM="xterm-256color")
        copied_binary = root / "shui binary ' $special"
        shutil.copy2(binary, copied_binary)
        binary = str(copied_binary)
        names = ["xdg-terminal-exec", "sensible-terminal", "x-terminal-emulator"]
        launcher_source = f"#!{sys.executable}\n" + r'''
import fcntl, json, os, pathlib, pty, select, struct, subprocess, sys, termios, time
with open(os.environ["SHUI_TEST_TERMINAL_LOG"], "a") as file:
    file.write(json.dumps({"launcher": pathlib.Path(sys.argv[0]).name,
                           "args": sys.argv[1:], "stdin": os.read(0, 1).hex()}) + "\n")
command = sys.argv[2:]
mode = os.environ.get("SHUI_TEST_TERMINAL_MODE", "record")
if mode == "fail":
    sys.exit(7)
if mode == "exec":
    os.chdir(os.environ["SHUI_TEST_TERMINAL_CWD"])
    os.execv(command[0], command)
if mode == "pty":
    if os.environ.get("SHUI_TEST_UNGUARDED"):
        command.remove("--no-terminal")
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))
    child = subprocess.Popen(command, stdin=slave, stdout=slave, stderr=slave,
                             cwd=os.environ["SHUI_TEST_TERMINAL_CWD"])
    output = bytearray()
    try:
        deadline = time.monotonic() + 3
        while b"SHUI" not in output and time.monotonic() < deadline:
            if select.select([master], [], [], 0.05)[0]:
                output.extend(os.read(master, 65536))
            if child.poll() is not None:
                break
        assert b"SHUI" in output, output
        os.write(master, b"\x03")
        assert child.wait(timeout=2) == 0, output
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
        os.close(master)
        os.close(slave)
'''
        for name in names:
            path = launchers / name
            path.write_text(launcher_source, encoding="utf-8")
            path.chmod(0o755)

        def launches():
            entries = [json.loads(line) for line in log.read_text(encoding="utf-8").splitlines()] if log.exists() else []
            log.unlink(missing_ok=True)
            return entries

        for display in ("DISPLAY", "WAYLAND_DISPLAY"):
            env[display] = ":test" if display == "DISPLAY" else "wayland-test"
            for args in [(), ("prod database",), ("--", "-prod"),
                         ("--", "--no-terminal"), ("' $(touch injected) ; $HOME",)]:
                result = run(*args)
                assert result.returncode == 0, result.stderr
                entry, = launches()
                assert entry["launcher"] == names[0]
                assert entry["stdin"] == ""
                assert entry["args"] == ["--", "/bin/sh", "-c",
                    'cd -- "$1" && shift && exec "$@"', "shui-terminal",
                    os.getcwd(), binary, "--no-terminal", *args], entry
                assert result.stdout == ""
            env.pop(display)

        for display in ({}, {"DISPLAY": "", "WAYLAND_DISPLAY": ""}):
            env.update(display)
            result = run()
            assert result.returncode != 0 and "requires a terminal" in result.stderr
            assert not launches()
        env["DISPLAY"] = ":test"
        assert run("--no-terminal").returncode != 0
        assert not launches()
        env["XDG_CONFIG_HOME"] = str(root / "uncreated-settings")
        assert run().returncode == 0
        assert len(launches()) == 1
        assert not (root / "uncreated-settings").exists(), "launcher created config before getting a TTY"
        env["XDG_CONFIG_HOME"] = target_path(root / "settings")
        for args in [("--help",), ("--version",), ("--dump",), ("--config-path",),
                     ("--check-config",), ("--check-config", str(default_config))]:
            result = run(*args)
            assert result.returncode == 0, result.stderr
            assert not launches(), args
        assert run("--unknown").returncode != 0
        assert not launches()
        assert not marker.exists()

        # A failed executable can fall back, but a started launcher's failure
        # must never create a second window or rerun a connection.
        (launchers / names[0]).chmod(0o644)
        assert run().returncode == 0
        assert launches()[0]["launcher"] == names[1]
        (launchers / names[0]).chmod(0o755)
        env["SHUI_TEST_TERMINAL_MODE"] = "fail"
        assert run().returncode == 7
        assert len(launches()) == 1
        env["SHUI_TEST_TERMINAL_MODE"] = "exec"
        result = run()
        assert result.returncode != 0 and "requires a terminal" in result.stderr
        assert len(launches()) == 1, "launcher without a TTY caused recursion"
        env.pop("SHUI_TEST_TERMINAL_MODE")

        for removed, expected in zip(names, names[1:]):
            (launchers / removed).unlink()
            assert run().returncode == 0
            entry, = launches()
            assert entry["launcher"] == expected and entry["args"][0] == "-e"
        (launchers / names[-1]).unlink()
        result = run()
        assert result.returncode != 0 and "no desktop terminal launcher" in result.stderr
        assert not launches()

        # End-to-end handoff to a real PTY, preserving cwd and literal arguments.
        path = launchers / names[0]
        path.write_text(launcher_source, encoding="utf-8")
        path.chmod(0o755)
        workdir = root / "working dir ' $(touch injected)"
        workdir.mkdir()
        default_config.write_text(json.dumps({"default": "SSH", "profiles": [{
            "name": "SSH", "command": "/bin/true %h",
            "available_command": 'test "$PWD" = ' + shlex.quote(str(workdir))}]}), encoding="utf-8")
        env["SHUI_TEST_TERMINAL_MODE"] = "pty"
        for unguarded in ("", "1"):
            env["SHUI_TEST_UNGUARDED"] = unguarded
            result = subprocess.run([binary, "' $(touch injected) ; $HOME"], env=env,
                                    cwd=workdir, capture_output=True, text=True, timeout=8)
            assert result.returncode == 0, result.stderr
            assert len(launches()) == 1, "existing TTY opened another terminal"
            assert not (workdir / "injected").exists()
            assert not (server_dir / "injected").exists()
        env.pop("SHUI_TEST_TERMINAL_MODE")

        # Either redirected stream is sufficient to require a new terminal.
        master, slave = pty.openpty()
        try:
            for stdin, stdout in [(slave, subprocess.PIPE), (subprocess.DEVNULL, slave)]:
                result = subprocess.run([binary], env=env, stdin=stdin, stdout=stdout,
                                        stderr=subprocess.PIPE, timeout=15 if wine else 5)
                assert result.returncode == 0, result.stderr
                assert len(launches()) == 1
        finally:
            os.close(master)
            os.close(slave)
        print("PASS: Linux desktop terminal selection, literal arguments, recursion guard, and PTY handoff")
print("PASS: CLI validation, discovery, environment independence, and terminal preflight")

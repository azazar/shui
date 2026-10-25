"""Real SSH, dependency installs and setup recipes, wholly inside one test container."""

import errno
from datetime import datetime, timezone
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import shlex
import socket
import struct
import subprocess
import termios
import time
import traceback

FIXTURES = Path("/tests/fixtures")
RESULTS = Path("/results")
RESULTS.mkdir(exist_ok=True)
CASES = []
SSH = ["ssh", "-p", "2222", "-o", "BatchMode=yes", "remote@shui-docker"]


def remote(command, check=True):
    result = subprocess.run(SSH + [command], capture_output=True, text=True, timeout=20)
    if check and result.returncode:
        raise AssertionError(f"remote command failed: {command}\n{result.stderr}")
    return result


def available(tool):
    command = f"command -v {shlex.quote(tool)} >/dev/null 2>&1"
    if tool == "zellij":
        command += ' || test -x "$HOME/.local/bin/zellij"'
    return remote(command, check=False).returncode == 0


def interactive(name, command, *, consent="y", finish=None, expected=0, timeout=600):
    """Respond only to known fixture/password/install prompts; preserve a real tty."""
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 120, 0, 0))

    def controlling_tty():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    process = subprocess.Popen(
        command,
        stdin=slave,
        stdout=slave,
        stderr=slave,
        preexec_fn=controlling_tty,
        env=dict(os.environ, TERM="xterm-256color"),
    )
    os.close(slave)
    log = (RESULTS / f"{name}.log").open("wb", buffering=0)
    transcript = bytearray()
    prompts = 0
    cursor = 0
    finished = finish is None
    deadline = time.monotonic() + timeout
    next_probe = time.monotonic() + 1
    responses = [
        (re.compile(r"(?i)Enter (?:same )?passphrase[^\r\n]*:"), b"\n", False),
        (
            re.compile(r"(?i)(?:password|Password)[^\r\n]*:"),
            b"shui-test-password\n",
            False,
        ),
        (re.compile(r"(?i)Install[^\r\n]*\[y/n\]"), (consent + "\n").encode(), True),
        (
            re.compile(
                r"(?i)(?:Do you want|Proceed|Is this ok|Continue)[^\r\n]*\[[^\]\r\n]*[yn][^\]\r\n]*\](?:[^\r\n]*:)?"
            ),
            b"y\n",
            False,
        ),
    ]
    try:
        while True:
            if time.monotonic() > deadline:
                raise TimeoutError(f"{name}: timed out after {timeout}s")
            if select.select([master], [], [], 0.1)[0]:
                try:
                    chunk = os.read(master, 65536)
                except OSError as error:
                    if error.errno != errno.EIO:
                        raise
                    chunk = b""
                transcript.extend(chunk)
                log.write(chunk)
                if len(transcript) > 8 * 1024 * 1024:
                    raise AssertionError(
                        f"{name}: excessive terminal output; see {log.name}"
                    )
                text = transcript.decode(errors="replace")
                # Handle one prompt at a time, even when SSH splits writes across reads.
                matches = [
                    (pattern.search(text, cursor), answer, install)
                    for pattern, answer, install in responses
                ]
                matches = [
                    (match, answer, install)
                    for match, answer, install in matches
                    if match
                ]
                if matches:
                    match, answer, install = min(
                        matches, key=lambda item: item[0].start()
                    )
                    cursor = match.end()
                    prompts += int(install)
                    os.write(master, answer)
            if process.poll() is not None:
                # Drain the final output before evaluating assertions.
                while select.select([master], [], [], 0)[0]:
                    try:
                        chunk = os.read(master, 65536)
                    except OSError:
                        break
                    if not chunk:
                        break
                    transcript.extend(chunk)
                    log.write(chunk)
                break
            if not finished and time.monotonic() >= next_probe:
                finished = finish(master, transcript.decode(errors="replace"))
                next_probe = time.monotonic() + 1
        plain = re.sub(
            r"\x1b\[[0-?]*[ -/]*[@-~]", "", transcript.decode(errors="replace")
        )
        assert process.returncode == expected, (
            f"{name}: exit {process.returncode}, expected {expected}\n"
            "Terminal output (tail):\n" + plain[-4000:]
        )
        assert finished, f"{name}: exited before the application was exercised"
        CASES.append({"case": name, "status": "passed", "install_prompts": prompts})
        print(f"PASS {name} (installation prompts: {prompts})", flush=True)
        return transcript.decode(errors="replace"), prompts
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        os.close(master)
        log.close()


def script(name, fixture=None, **kwargs):
    return interactive(name, ["sh", str(FIXTURES / f"{fixture or name}.sh")], **kwargs)


def initialize(listen="127.0.0.1"):
    subprocess.run(["ssh-keygen", "-A"], check=True)
    config = Path("/tests/sshd_config")
    config.write_text(f"""Port 2222
ListenAddress {listen}
HostKey /etc/ssh/ssh_host_ed25519_key
PasswordAuthentication yes
KbdInteractiveAuthentication no
UsePAM no
PermitRootLogin no
PidFile /run/shui-test-sshd.pid
Subsystem sftp internal-sftp
AcceptEnv http_proxy https_proxy no_proxy HTTP_PROXY HTTPS_PROXY NO_PROXY
SetEnv ENV=/home/remote/.profile LANG=C.UTF-8
""")
    effective = subprocess.run(
        ["/usr/sbin/sshd", "-T", "-f", str(config)],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    if "persourcepenalties " in effective.lower():
        # This loopback fixture deliberately rejects keys; don't rate-limit later cases.
        with config.open("a") as file:
            file.write("PerSourcePenalties no\n")
    Path("/root/.ssh/config").write_text("""Host shui-docker
  HostName 127.0.0.1
  Port 1
  User remote
  StrictHostKeyChecking no
  UserKnownHostsFile /root/.ssh/known_hosts
  IdentityFile /root/.ssh/id_ed25519
  ConnectTimeout 10
  SendEnv http_proxy https_proxy no_proxy HTTP_PROXY HTTPS_PROXY NO_PROXY
""")
    Path("/root/.ssh/config").chmod(0o600)
    # A recognizable interactive shell prompt for SSH/Bash and multiplexers.
    for name in [".profile", ".bash_profile", ".bashrc"]:
        path = Path("/home/remote") / name
        path.write_text("case $- in *i*) PS1='SHUI_READY> '; export PS1 ;; esac\n")
        subprocess.run(["chown", "remote:remote", str(path)], check=True)
    server_log = (RESULTS / "sshd.log").open("w")
    server = subprocess.Popen(
        ["/usr/sbin/sshd", "-D", "-e", "-f", str(config)],
        stdout=server_log,
        stderr=server_log,
    )
    for _ in range(100):
        if server.poll() is not None:
            raise RuntimeError("sshd exited during startup")
        try:
            with socket.create_connection(("127.0.0.1", 2222), timeout=0.1):
                return server, server_log
        except OSError:
            time.sleep(0.1)
    raise TimeoutError("sshd did not start")


def shell_finish(master, text):
    if "SHUI_READY>" not in text:
        return False
    os.write(master, b"printf '\\nSHUI_SHELL_OK:%s\\n' \"$HOME\"; exit\n")
    return True


def profile(name, finish, tool=None):
    script(f"{name}-prepare")
    missing = tool is not None and not available(tool)
    if missing:
        _, prompts = script(f"{name}-decline", name, consent="n", expected=1)
        assert prompts == 1, f"{name}: missing dependency did not offer installation"
        assert not available(tool), f"{name}: installed despite declining"
    output, prompts = script(name, finish=finish)
    if missing:
        assert prompts == 1 and available(tool), f"{name}: dependency was not installed"
    if name in ["ssh", "bash"]:
        assert "\r\nSHUI_SHELL_OK:/home/remote\r\n" in output, output[-2000:]
    # A second launch must reuse the installed dependency without prompting.
    _, prompts = script(f"{name}-reuse", name, finish=finish)
    assert prompts == 0, f"{name}: repeated the dependency installation"


def run():
    server, server_log = initialize()
    try:
        # Generate through the actual recipe, accepting an empty test-only passphrase.
        script("auth-generate")
        assert Path("/root/.ssh/id_ed25519").exists()
        remote("true")  # Passwordless authentication must now work.
        script("auth-existing", "auth")
        authorized = remote('cat "$HOME/.ssh/authorized_keys"').stdout.splitlines()
        public = Path("/root/.ssh/id_ed25519.pub").read_text().strip()
        assert authorized.count(public) == 1, "ssh-copy-id duplicated the key"
        # The recipe must protect an already existing keypair.
        before = Path("/root/.ssh/id_ed25519").read_bytes()
        script("auth-no-overwrite", "auth-generate", expected=1)
        assert Path("/root/.ssh/id_ed25519").read_bytes() == before

        for choice, seconds in [
            ("8-hours", 28800),
            ("day", 86400),
            ("week", 604800),
            ("month", 2592000),
            ("year", 31536000),
            ("never", 0),
        ]:
            started = int(remote("date +%s").stdout)
            script(f"auth-expiry-{choice}")
            # Reading over a new BatchMode connection also proves sshd accepts the option.
            authorized = remote('cat "$HOME/.ssh/authorized_keys"').stdout
            match = re.search(r'expiry-time="(\d{14}Z)"', authorized)
            if seconds:
                assert match, f"{choice}: missing expiry-time"
                deadline = int(
                    datetime.strptime(match[1], "%Y%m%d%H%M%SZ")
                    .replace(tzinfo=timezone.utc)
                    .timestamp()
                )
                ended = int(remote("date +%s").stdout)
                assert started + seconds <= deadline <= ended + seconds
            else:
                assert "expiry-time=" not in authorized

        # Demonstrate enforcement, then renew via the password-authenticated recipe.
        authorized_file = Path("/home/remote/.ssh/authorized_keys")
        authorized_file.write_text('expiry-time="20000101000000Z" ' + public + "\n")
        rejected = remote("true", check=False)
        assert rejected.returncode == 255 and "Permission denied" in rejected.stderr
        CASES.append({"case": "auth-expired-rejected", "status": "passed"})
        print("PASS auth-expired-rejected", flush=True)
        script("auth-expired-renew", "auth-expiry-day")
        remote("true")
        script("auth-expiry-reset", "auth-expiry-never")
        assert "expiry-time=" not in remote('cat "$HOME/.ssh/authorized_keys"').stdout
        assert not list(Path("/home/remote/.ssh").glob(".shui-expiry.*"))

        dotfiles = Path("/root/src/dotfiles")
        dotfiles.mkdir(parents=True)
        (dotfiles / "install.sh").write_text(
            '#!/bin/sh\nset -eu\ncmp .fixture expected\nprintf "dotfiles-installed\\n" > "$HOME/.dotfiles-installed"\n'
        )
        (dotfiles / ".fixture").write_text("hidden file copied intact\n")
        (dotfiles / "expected").write_bytes((dotfiles / ".fixture").read_bytes())
        script("dotfiles")
        assert (
            remote('cat "$HOME/.dotfiles-installed"').stdout.strip()
            == "dotfiles-installed"
        )
        remote('test -f "$HOME/src/dotfiles/.fixture"')

        definition = Path("/tests/fixture.terminfo")
        definition.write_text(
            "shui-docker|integration test terminal,\n cols#80, lines#24, bel=^G, clear=\\E[H\\E[2J,\n"
        )
        # Some ncurses builds ignore root's ~/.terminfo. Install the client's
        # fixture in the system database, and check the remote's private database
        # explicitly so a system fallback cannot make the copy test pass.
        subprocess.run(["tic", "-x", str(definition)], check=True)
        remote_definition = 'infocmp -x -A "$HOME/.terminfo" shui-docker'
        assert remote(remote_definition, check=False).returncode != 0
        os.environ["TERM"] = "shui-docker"
        # interactive() sets TERM for terminal apps; explicitly set it for this recipe.
        interactive(
            "terminfo", ["env", "TERM=shui-docker", "sh", str(FIXTURES / "terminfo.sh")]
        )
        assert "cols#80" in remote(remote_definition).stdout
        assert not list(Path("/tmp").glob("shui-terminfo.*"))
        os.environ["TERM"] = "xterm-256color"

        profile("ssh", shell_finish)
        profile("bash", shell_finish, "bash")

        # tmux and btop must not be accidentally preinstalled in the test image.
        for tool in ["tmux", "btop", "zellij"]:
            assert not available(tool), (
                f"{tool} was preinstalled; auto-install coverage would be lost"
            )

        def detach_tmux(master, text):
            if (
                "SHUI_READY>" not in text
                or remote("tmux has-session -t main", check=False).returncode
            ):
                return False
            remote("tmux detach-client -s main")
            return True

        profile("tmux", detach_tmux, "tmux")
        remote("tmux has-session -t main")
        remote("tmux kill-server")

        configs = []

        def close_monitor(master, text):
            if remote("pgrep -u remote -x btop", check=False).returncode:
                return False
            matches = list(Path("/tmp").glob("shui-monitor.*/btop/btop.conf"))
            if not matches:
                return False
            config = matches[0]
            assert 'shown_boxes = "cpu mem net"' in config.read_text()
            configs.append(config)
            os.write(master, b"q")
            return True

        profile("monitor", close_monitor, "btop")
        assert len(configs) == 2 and all(not path.exists() for path in configs), (
            "Monitor did not remove temporary configs"
        )

        session_names = []

        def live_zellij_sessions():
            result = remote(
                '"$HOME/.local/bin/zellij" list-sessions --no-formatting', check=False
            )
            live = [
                line.split(" [Created ", 1)[0]
                for line in result.stdout.splitlines()
                if "(EXITED" not in line
            ]
            return live if result.returncode == 0 else []

        def detach_zellij(master, text):
            if "SHUI_READY>" not in text:
                return False
            live = live_zellij_sessions()
            if not live:
                return False
            assert len(live) == 1, live
            name = live[0]
            client = '"$HOME/.local/bin/zellij" --session ' + shlex.quote(name)
            tabs = remote(client + " action query-tab-names").stdout.splitlines()
            assert len(tabs) == 1, f"Zellij opened extra tabs: {tabs}"
            panes = json.loads(remote(client + " action list-panes --json").stdout)
            visible = [pane for pane in panes if not pane["is_suppressed"]]
            assert len(visible) == 1, panes
            assert not visible[0]["is_plugin"] and not visible[0]["is_held"], panes
            # Detach the actual terminal client; a separate CLI action has no client ID.
            os.write(master, b"\x0fd")  # Zellij's default Ctrl+O, d binding.
            session_names.append(name)
            return True

        profile("zellij", detach_zellij, "zellij")
        assert len(session_names) == 2 and session_names[0] == session_names[1], (
            "Zellij did not attach to the existing session"
        )
        remote('"$HOME/.local/bin/zellij" kill-all-sessions --yes')
        # Wait for asynchronous server shutdown before testing a fresh launch.
        deadline = time.monotonic() + 10
        while live_zellij_sessions():
            assert time.monotonic() < deadline, "Zellij did not stop"
            time.sleep(0.1)
        # Exited sessions must not be resurrected with suspended command prompts.
        script("zellij-after-exit", "zellij", finish=detach_zellij)
        assert session_names[-1] != session_names[0], (
            "Zellij resurrected the exited session"
        )
        remote('"$HOME/.local/bin/zellij" kill-all-sessions --yes')
        assert not list(Path("/home/remote/.local/bin").glob(".shui-zellij.*"))

        # Sudo recipes run last so their authentication policy cannot mask
        # dependency-installation failures in the profile scenarios above.
        distro = dict(
            line.split("=", 1)
            for line in Path("/etc/os-release").read_text().splitlines()
            if "=" in line
        )["ID"].strip('"')
        pam_source = next(
            (
                path
                for path in [Path("/etc/pam.d/sudo"), Path("/usr/lib/pam.d/sudo")]
                if path.exists()
            ),
            None,
        )
        pam_before = pam_source.read_bytes() if pam_source else None
        keys_before = Path("/home/remote/.ssh/authorized_keys").read_bytes()
        agent = subprocess.check_output(["ssh-agent", "-s"], text=True)
        agent_env = dict(re.findall(r"(SSH_AUTH_SOCK|SSH_AGENT_PID)=([^;]+);", agent))
        os.environ.update(agent_env)
        try:
            subprocess.run(["ssh-add", "/root/.ssh/id_ed25519"], check=True)
            if distro in ("alpine", "arch", "fedora"):
                script("sudo-agent-unavailable", "sudo-agent", expected=1)
                assert not Path("/etc/sudoers.d/ssh-agent-auth").exists()
                if pam_before is not None:
                    assert pam_source.read_bytes() == pam_before
            else:
                script("sudo-agent")
                script("sudo-agent-repeat", "sudo-agent")
                Path("/etc/sudoers.d/shui-test").write_text(
                    "remote ALL=(ALL:ALL) ALL\n"
                )
                authenticated = subprocess.run(
                    SSH[:1] + ["-A"] + SSH[1:] + ["sudo -k; sudo -S true"],
                    input=b"",
                    capture_output=True,
                    timeout=20,
                )
                assert authenticated.returncode == 0, authenticated.stderr
                unauthenticated = subprocess.run(
                    SSH + ["sudo -k; sudo -S true"],
                    input=b"",
                    capture_output=True,
                    timeout=20,
                )
                assert unauthenticated.returncode != 0
                fallback = subprocess.run(
                    SSH + ["sudo -k; sudo -S id -u"],
                    input=b"shui-test-password\n",
                    capture_output=True,
                    timeout=20,
                )
                assert fallback.returncode == 0 and fallback.stdout.strip() == b"0", (
                    fallback.stderr
                )
                subprocess.run(["ssh-add", "-D"], check=True)
                subprocess.run(
                    [
                        "ssh-keygen",
                        "-q",
                        "-t",
                        "ed25519",
                        "-N",
                        "",
                        "-f",
                        "/root/.ssh/untrusted-agent",
                    ],
                    check=True,
                )
                subprocess.run(["ssh-add", "/root/.ssh/untrusted-agent"], check=True)
                wrong_agent = subprocess.run(
                    SSH[:1] + ["-A"] + SSH[1:] + ["sudo -k; sudo -S true"],
                    input=b"",
                    capture_output=True,
                    timeout=20,
                )
                assert wrong_agent.returncode != 0
                Path("/etc/sudoers.d/shui-test").write_text(
                    "remote ALL=(ALL) NOPASSWD: ALL\n"
                )
            assert Path("/home/remote/.ssh/authorized_keys").read_bytes() == keys_before
        finally:
            subprocess.run(["ssh-agent", "-k"], check=True)
            for name in agent_env:
                os.environ.pop(name, None)
        script("sudo-nopasswd")
        group = "sudo" if distro in ("debian", "ubuntu") else "wheel"
        subprocess.run(["usermod", "-aG", group, "remote"], check=True)
        Path("/etc/sudoers.d/shui-test").unlink()
        assert remote("sudo -k; sudo -n id -u").stdout.strip() == "0"
        script("sudo-nopasswd-repeat", "sudo-nopasswd")
        assert remote("sudo -k; sudo -n id -u").stdout.strip() == "0"

    finally:
        server.terminate()
        server.wait(timeout=10)
        server_log.close()


if __name__ == "__main__":
    try:
        run()
    except Exception as error:
        CASES.append({"case": "suite", "status": "failed", "error": str(error)})
        traceback.print_exc()
        raise
    finally:
        (RESULTS / "report.json").write_text(json.dumps(CASES, indent=2) + "\n")

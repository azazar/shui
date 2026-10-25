"""Behavioral service-recipe tests inside a disposable, booted systemd container."""

import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import time
import traceback

from container import CASES, RESULTS, initialize, script
from guest_logs import check_guest_recursion


def run(*args, **kwargs):
    result = subprocess.run(
        args,
        capture_output=True,
        text=True,
        timeout=kwargs.pop("timeout", 600),
        **kwargs,
    )
    assert result.returncode == 0, f"{args}: {result.stdout}\n{result.stderr}"
    return result.stdout.strip()


def active(unit):
    run("systemctl", "is-active", "--quiet", unit)


def probe(port, source="192.0.2.2", udp=False):
    destination = "2001:db8::1" if ":" in source else "192.0.2.1"
    code = """import socket, sys
source, destination, port, udp = sys.argv[1:]
s = socket.socket(socket.AF_INET6 if ':' in source else socket.AF_INET,
                  socket.SOCK_DGRAM if udp == 'yes' else socket.SOCK_STREAM)
s.settimeout(2)
s.bind((source, 0))
s.connect((destination, int(port)))
if udp == 'yes':
    s.send(b'probe')
    assert s.recv(100) == b'probe'
s.close()
"""
    return (
        subprocess.run(
            [
                "ip",
                "netns",
                "exec",
                "client",
                "python3",
                "-c",
                code,
                source,
                destination,
                str(port),
                "yes" if udp else "no",
            ],
            capture_output=True,
            timeout=5,
        ).returncode
        == 0
    )


def containers():
    script("containers", timeout=900)
    active("docker")
    run("systemctl", "is-enabled", "docker")
    # Build a tiny local rootfs: executing a process proves the engine works,
    # without depending on an external registry or a particular image tag.
    root = Path("/tests/rootfs")
    root.mkdir()
    executable = Path("/bin/sh").resolve()
    paths = {
        str(executable),
        *re.findall(r"(/[\w/+.\-]+)", run("ldd", str(executable))),
    }
    for name in paths:
        path = root / name.lstrip("/")
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(name, path)
    (root / "bin").mkdir(exist_ok=True)
    if executable != Path("/bin/sh"):
        (root / "bin/sh").symlink_to(str(executable))
    with tarfile.open("/tests/rootfs.tar", "w") as archive:
        for path in root.iterdir():
            archive.add(path, arcname=path.name)
    run("docker", "import", "/tests/rootfs.tar", "shui-fixture")
    assert (
        run(
            "docker",
            "run",
            "--rm",
            "--network=none",
            "shui-fixture",
            "/bin/sh",
            "-c",
            "echo engine-works",
        )
        == "engine-works"
    )
    script("podman", timeout=900)
    assert (
        run(
            "podman",
            "--runtime=crun",
            "run",
            "--cgroups=disabled",
            "--rm",
            "--network=none",
            "--rootfs",
            str(root),
            "/bin/sh",
            "-c",
            "echo engine-works",
        )
        == "engine-works"
    )
    CASES.append({"case": "container-engines-execute-processes", "status": "passed"})


def updates():
    distro = run("sh", "-c", '. /etc/os-release; echo "$ID"')
    supported = distro in (
        "debian",
        "ubuntu",
        "fedora",
        "rhel",
        "rocky",
        "almalinux",
        "opensuse-leap",
        "sles",
    )
    script("security-updates", expected=0 if supported else 1)
    if not supported:
        assert (
            subprocess.run(
                ["systemctl", "is-enabled", "shui-security-updates.timer"],
                capture_output=True,
            ).returncode
            != 0
        )
        return
    active("shui-security-updates.timer")
    run("systemctl", "is-enabled", "shui-security-updates.timer")
    assert run(
        "systemctl",
        "show",
        "--value",
        "-p",
        "NextElapseUSecRealtime",
        "shui-security-updates.timer",
    )
    run(
        "systemd-analyze",
        "verify",
        "/etc/systemd/system/shui-security-updates.service",
        "/etc/systemd/system/shui-security-updates.timer",
    )
    if distro in ("debian", "ubuntu"):
        configuration = run("apt-config", "dump")
        origins = [
            line
            for line in configuration.splitlines()
            if re.match(
                r"Unattended-Upgrade::(?:Allowed-Origins|Origins-Pattern):: ", line
            )
        ]
        assert origins and all("-security" in line for line in origins), origins
        assert 'Unattended-Upgrade::Automatic-Reboot "false";' in configuration
    # Actually run the scheduled service, rather than just checking unit text.
    run("systemctl", "start", "shui-security-updates.service", timeout=900)
    assert (
        run(
            "systemctl",
            "show",
            "--value",
            "-p",
            "Result",
            "shui-security-updates.service",
        )
        == "success"
    )
    CASES.append({"case": "security-update-service-completes", "status": "passed"})


def journals():
    script("journald-invalid", expected=1)
    active("systemd-journald")
    script("journald-small")
    # Exercise log ingestion, rotation and the configured storage ceiling.
    run(
        "systemd-cat",
        "-t",
        "shui-fixture",
        input="".join(os.urandom(20000).hex() + "\n" for _ in range(1000)),
    )
    run("journalctl", "--sync")
    script("journald-reapply", "journald-small")
    run("systemd-cat", "-t", "shui-result", input="journal-works\n")
    run("journalctl", "--sync")
    assert "journal-works" in run(
        "journalctl", "-t", "shui-result", "-o", "cat", "--no-pager"
    )
    size = sum(
        path.stat().st_blocks * 512
        for base in ("/var/log/journal", "/run/log/journal")
        for path in Path(base).rglob("*.journal*")
        if path.is_file()
    )
    # journald retains its active file in addition to archived files being vacuumed.
    assert size <= 32 * 1024 * 1024, size
    CASES.append({"case": "journald-ingestion-and-storage-bound", "status": "passed"})


def log_cleanup():
    cache = Path("/home/remote/.cache")
    cache.mkdir(exist_ok=True)
    old = cache / "old-cache"
    recent = cache / "recent-cache"
    old.write_text("old cached content")
    recent.write_text("recent cached content")
    run("chown", "-R", "remote:remote", str(cache))
    earlier = time.time() - 4 * 86400
    os.utime(old, (earlier, earlier))
    outside = Path("/tests/cache-link-target")
    outside.write_text("preserve outside cache")
    (cache / "linked-cache").symlink_to(outside)
    archive = Path("/var/log/shui-history.log.1")
    archive.write_text("expired archived log")
    os.utime(archive, (earlier, earlier))
    latest = Path("/var/log/shui-history.log.2")
    latest.write_text("recent archived log")
    live = Path("/var/log/shui-active.log")
    live.write_bytes(b"x" * (20 * 1024 * 1024))
    Path("/etc/logrotate.d/shui-fixture").write_text(
        "/var/log/shui-active.log {\n copytruncate\n missingok\n}\n"
    )
    docker_config = Path("/etc/docker/daemon.json")
    configuration = (
        json.loads(docker_config.read_text()) if docker_config.exists() else {}
    )
    configuration["labels"] = ["shui-preserve=true"]
    docker_config.parent.mkdir(exist_ok=True)
    docker_config.write_text(json.dumps(configuration))
    script("logs-retention")
    assert not old.exists() and recent.exists()
    assert not archive.exists() and latest.exists()
    assert live.stat().st_size <= 10 * 1024 * 1024
    assert outside.read_text() == "preserve outside cache"
    configuration = json.loads(docker_config.read_text())
    assert configuration["labels"] == ["shui-preserve=true"]
    assert configuration["log-opts"]["max-size"] == "10485760"
    assert configuration["log-opts"]["max-file"] == "3"
    if shutil.which("podman"):
        assert (
            "log_size_max=10485760"
            in Path(
                "/etc/containers/containers.conf.d/90-shui-logging.conf"
            ).read_text()
        )
    script("logs-cleanup")
    assert not recent.exists() and not latest.exists()
    assert live.exists() and outside.read_text() == "preserve outside cache"
    check_guest_recursion()
    CASES.append(
        {"case": "host-and-nested-agent-log-and-cache-cleanup", "status": "passed"}
    )


def firewall():
    run("ip", "netns", "add", "client")
    run("ip", "link", "add", "server0", "type", "veth", "peer", "name", "client0")
    run("ip", "link", "set", "client0", "netns", "client")
    run("ip", "addr", "add", "192.0.2.1/24", "dev", "server0")
    run("ip", "-6", "addr", "add", "2001:db8::1/64", "dev", "server0", "nodad")
    run("ip", "link", "set", "server0", "up")
    for ip in ("192.0.2.2/24", "192.0.2.3/24", "2001:db8::2/64", "2001:db8::3/64"):
        run(
            "ip",
            "-n",
            "client",
            "addr",
            "add",
            ip,
            "dev",
            "client0",
            *(["nodad"] if ":" in ip else []),
        )
    run("ip", "-n", "client", "link", "set", "client0", "up")
    run("ip", "-n", "client", "link", "set", "lo", "up")
    listener = subprocess.Popen(
        [
            "python3",
            "-c",
            """import socket, threading, time
def serve(port, udp):
    s = socket.socket(socket.AF_INET6, socket.SOCK_DGRAM if udp else socket.SOCK_STREAM)
    s.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 0)
    s.bind(('::', port))
    if not udp: s.listen()
    while True:
        if udp:
            data, address = s.recvfrom(100); s.sendto(data, address)
        else:
            connection, _ = s.accept(); connection.close()
for port, udp in [(8080, False), (8081, False), (5353, True), (5354, True)]:
    threading.Thread(target=serve, args=(port, udp), daemon=True).start()
time.sleep(600)
""",
        ]
    )
    try:
        for _ in range(30):
            if probe(8080):
                break
            time.sleep(0.1)
        assert probe(2222) and probe(8081) and probe(5354, udp=True)
        for fixture in (
            "firewall-invalid-ip",
            "firewall-invalid-port",
            "firewall-ssh-bypass",
        ):
            script(fixture, expected=1)
            assert probe(2222) and probe(8081)
        script("firewall-restricted")
        for source in ("192.0.2.2", "2001:db8::2"):
            assert probe(2222, source), source
            assert probe(8080, source), source
            assert probe(5353, source, udp=True), source
            assert not probe(8081, source), source
            assert not probe(5354, source, udp=True), source
        assert not probe(2222, "192.0.2.3")
        assert not probe(2222, "2001:db8::3")
        script("firewall-changed")
        assert probe(2222, "192.0.2.3")
        assert not probe(2222, "192.0.2.2")
        assert not probe(2222, "2001:db8::2")
        # Blank SSH sources restores access; previously allowed ports stay allowed.
        script("firewall")
        assert probe(2222, "192.0.2.2") and probe(2222, "2001:db8::3")
        assert probe(8080) and not probe(8081)
        CASES.append(
            {
                "case": "firewall-enforces-ipv4-ipv6-tcp-udp-and-edits",
                "status": "passed",
            }
        )
    finally:
        listener.terminate()
        listener.wait(timeout=5)
        run("ip", "netns", "delete", "client")


def main():
    for _ in range(100):
        if Path("/run/systemd/system").exists():
            break
        time.sleep(0.1)
    subprocess.run(
        ["systemctl", "is-system-running", "--wait"], timeout=90, capture_output=True
    )
    active("systemd-journald")
    # Package installation tests must not race the image's background apt jobs.
    subprocess.run(
        ["systemctl", "stop", "apt-daily.timer", "apt-daily-upgrade.timer"],
        capture_output=True,
    )
    Path("/run/sshd").mkdir(exist_ok=True)
    server, log = initialize("0.0.0.0\nListenAddress ::")
    try:
        script("auth-generate")
        failures = []
        for test in (journals, updates, containers, log_cleanup, firewall):
            try:
                test()
            except Exception:
                failures.append(test.__name__)
                traceback.print_exc()
        assert not failures, failures
        print("PASS all service recipe outcomes", flush=True)
    finally:
        (RESULTS / "services.json").write_text(json.dumps(CASES, indent=2))
        server.terminate()
        server.wait(timeout=5)
        log.close()


if __name__ == "__main__":
    main()

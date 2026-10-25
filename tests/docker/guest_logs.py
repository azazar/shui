"""Exercise actual QEMU guest-agent execution in two nested chroots.

The virsh fixture supplies discovery/transport only; unmodified qemu-ga executes
the shipped maintenance payload inside separate guest filesystems. This does not
claim to test libvirt domain discovery or a full VM boot.
"""

import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import sysconfig
import time

from container import script


def transport(socket, uuid):
    return f"""#!/usr/bin/python3
import json, socket, sys
if 'list' in sys.argv:
    print({uuid!r}); sys.exit(0)
request = json.loads(sys.argv[-1])
client = socket.socket(socket.AF_UNIX)
client.settimeout(15)
client.connect({socket!r})
client.sendall((json.dumps(request) + '\\n').encode())
reply = json.loads(client.makefile().readline())
if 'error' in reply:
    sys.exit(str(reply['error']))
print(json.dumps(reply))
"""


def executable(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    path.chmod(0o755)


def rootfs(root, identity):
    for directory in (
        "etc/logrotate.d",
        "etc/libvirt",
        "var/log/libvirt/qemu",
        "var/lib/logrotate",
        "run",
        "dev",
        "root/.cache",
        "usr/local/bin",
        "tmp",
    ):
        (root / directory).mkdir(parents=True, exist_ok=True)
    stdlib = Path(sysconfig.get_path("stdlib"))
    shutil.copytree(stdlib, root / str(stdlib).lstrip("/"), symlinks=False)
    programs = [
        sys.executable,
        "/bin/sh",
        shutil.which("logrotate"),
        shutil.which("qemu-ga"),
        *[
            shutil.which(tool)
            for tool in (
                "bash",
                "jq",
                "find",
                "timeout",
                "cat",
                "chmod",
                "dirname",
                "mktemp",
                "mv",
                "rm",
                "sed",
                "awk",
                "sort",
                "date",
                "base64",
                "sleep",
                "tail",
                "getent",
            )
        ],
    ]
    copied = set()

    def copy_binary(name):
        if name in copied:
            return
        copied.add(name)
        target = root / name.lstrip("/")
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(name, target)
        result = subprocess.run(
            ["ldd", name], capture_output=True, text=True, timeout=10
        )
        for dependency in re.findall(r"(/[\w/+.\-]+)", result.stdout):
            if dependency not in copied:
                copy_binary(dependency)

    for program in programs:
        copy_binary(program)
    python = root / "usr/bin/python3"
    if not python.exists():
        python.parent.mkdir(parents=True, exist_ok=True)
        python.symlink_to(sys.executable)
    for module in stdlib.rglob("*.so"):
        # The stdlib copy already includes these modules; collect their libraries.
        result = subprocess.run(
            ["ldd", str(module)], capture_output=True, text=True, timeout=10
        )
        for dependency in re.findall(r"(/[\w/+.\-]+)", result.stdout):
            copy_binary(dependency)
    (root / "etc/machine-id").write_text(identity * 32 + "\n")
    (root / "etc/passwd").write_text("root:x:0:0:root:/root:/bin/sh\n")
    (root / "etc/group").write_text("root:x:0:\n")
    (root / "etc/nsswitch.conf").write_text("passwd: files\ngroup: files\n")
    (root / "etc/logrotate.conf").write_text("include /etc/logrotate.d\n")
    (root / "var/log/libvirt/qemu/fixture.log").write_text("keep active VM log\n")
    (root / "var/log/libvirt/qemu/fixture.log.1").write_text("remove archived VM log\n")
    (root / "root/.cache/fixture").write_text("remove guest cache\n")
    os.mknod(root / "dev/null", 0o20666, os.makedev(1, 3))


def check_guest_recursion():
    outer = Path("/tests/shui-guests/one")
    inner = outer / "nested"
    rootfs(outer, "1")
    rootfs(inner, "2")
    host_transport = Path("/usr/local/bin/virsh")
    assert not host_transport.exists(), (
        "test must not replace an existing virsh executable"
    )
    executable(
        host_transport,
        transport(str(outer / "agent.sock"), "00000000-0000-0000-0000-000000000001"),
    )
    executable(
        outer / "usr/local/bin/virsh",
        transport("/nested/agent.sock", "00000000-0000-0000-0000-000000000002"),
    )
    processes = []
    logs = []
    try:
        for root in (outer, inner):
            log = (root / "agent.log").open("w")
            logs.append(log)
            process = subprocess.Popen(
                [
                    "chroot",
                    str(root),
                    shutil.which("qemu-ga"),
                    "-m",
                    "unix-listen",
                    "-p",
                    "/agent.sock",
                    "-f",
                    "/run/qemu-ga.pid",
                    "-t",
                    "/run",
                ],
                stdout=log,
                stderr=log,
            )
            processes.append(process)
            for _ in range(100):
                if (root / "agent.sock").exists():
                    break
                if process.poll() is not None:
                    raise AssertionError((root / "agent.log").read_text())
                time.sleep(0.05)
            assert (root / "agent.sock").exists()
        script("logs-guests", timeout=600)
        for root in (outer, inner):
            assert (
                root / "var/log/libvirt/qemu/fixture.log"
            ).read_text() == "keep active VM log\n"
            assert not (root / "var/log/libvirt/qemu/fixture.log.1").exists()
            assert not (root / "root/.cache/fixture").exists()
            assert (
                "max_size = 10485760"
                in (root / "etc/libvirt/virtlogd.conf").read_text()
            )
        # An inaccessible agent must produce an incomplete outcome, not success.
        processes[-1].terminate()
        processes[-1].wait(timeout=5)
        output, _ = script(
            "logs-guest-unavailable", "logs-guests", expected=1, timeout=600
        )
        assert "Incomplete log maintenance" in output
    finally:
        host_transport.unlink()
        for process in processes:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
        for log in logs:
            log.close()

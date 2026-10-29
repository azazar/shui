"""Offline checks of existing and checksum-verified portable Zellij launches."""

import hashlib
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import textwrap

repo = Path(__file__).resolve().parents[1]
config = (repo / "docs/config.example.yml").read_text()
block = config.split("  - name: Zellij\n", 1)[1].split("  - name:", 1)[0]
source = textwrap.dedent(block.split("    script: |\n", 1)[1])

with tempfile.TemporaryDirectory(prefix="shui-zellij-test-") as directory:
    root = Path(directory)
    bins = root / "bin"
    bins.mkdir()
    env = dict(os.environ, HOME=str(root), PATH=str(bins), TEST_LOG=str(root / "calls"))
    config_file = root / ".config/zellij/config.kdl"
    config_file.parent.mkdir(parents=True)
    config_file.write_text("simplified_ui true\n")
    for tool in ["curl", "wget", "sudo", "apt-get"]:
        path = bins / tool
        path.write_text('#!/bin/sh\necho unexpected > "$HOME/side-effect"; exit 99\n')
        path.chmod(0o755)

    missing = subprocess.run(
        ["/bin/sh", "-c", source], env=env, text=True, capture_output=True, timeout=10
    )
    assert missing.returncode != 0
    assert not (root / "side-effect").exists()
    assert not (root / ".local").exists()

    zellij = bins / "zellij"
    zellij.write_text("""#!/bin/sh
printf '%s\\n' "$*" > "$TEST_LOG"
IFS= read -r line
[ "$line" = terminal-input ] || exit 9
exit "${TEST_EXIT:-0}"
""")
    zellij.chmod(0o755)

    def launch(script=source, status=0):
        result = subprocess.run(
            ["/bin/sh", "-c", script],
            env=dict(env, TEST_EXIT=str(status)),
            input="terminal-input\n",
            text=True,
            capture_output=True,
            timeout=10,
        )
        assert result.returncode == status, result.stderr
        assert (root / "calls").read_text().splitlines() == [
            "attach --create options --show-startup-tips false --show-release-notes false"
        ]
        assert config_file.read_text() == "simplified_ui true\n"
        assert not (root / "side-effect").exists()

    for status in [0, 42]:
        launch(status=status)
    assert not (root / ".local").exists()

    # Non-login SSH PATH often omits ~/.local/bin. No download is needed here.
    local = root / ".local/bin"
    local.mkdir(parents=True)
    zellij.rename(local / "zellij")
    for status in [0, 42]:
        launch(status=status)

    # Use a real tarball and hashing tools; substitute only the release payload
    # and its pinned digest, so the test is offline and never runs downloaded code.
    payload = local / "zellij"
    digest = hashlib.sha256(payload.read_bytes()).hexdigest()
    archive = root / "fixture.tar.gz"
    with tarfile.open(archive, "w:gz") as tar:
        tar.add(payload, arcname="zellij")
    payload.unlink()
    portable = re.sub(r"expected=[0-9a-f]{64}", f"expected={digest}", source)
    temporary = root / "temporary"
    temporary.mkdir()
    env.update(TMPDIR=str(temporary), TEST_ARCHIVE=str(archive))
    checksum = "sha256sum" if shutil.which("sha256sum") else "shasum"
    for tool in ["tar", "gzip", checksum, "mktemp", "chmod", "rm", "cp"]:
        (bins / tool).symlink_to(shutil.which(tool))
    (bins / "uname").write_text(
        '#!/bin/sh\ncase "$1" in -s) echo Linux ;; -m) echo x86_64 ;; esac\n'
    )
    (bins / "uname").chmod(0o755)
    (bins / "curl").write_text('''#!/bin/sh
[ "${TEST_DOWNLOAD_FAIL:-0}" = 0 ] || exit 22
while [ "$#" -gt 0 ]; do
  if [ "$1" = --output ]; then cp "$TEST_ARCHIVE" "$2"; exit; fi
  shift
done
exit 99
''')
    for status in [0, 42]:
        launch(portable, status)
        assert not list(temporary.iterdir()), "temporary download was not removed"
        assert not payload.exists(), "portable launch must not install Zellij"

    for script, download_failure, message in [
        (source, "0", "checksum mismatch"),
        (portable, "1", "download failed"),
    ]:
        (root / "calls").unlink(missing_ok=True)
        result = subprocess.run(
            ["/bin/sh", "-c", script],
            env=dict(env, TEST_DOWNLOAD_FAIL=download_failure),
            text=True, capture_output=True, timeout=10,
        )
        assert result.returncode != 0
        assert message in result.stderr, result.stderr
        assert not (root / "calls").exists(), "unverified Zellij was executed"
        assert not list(temporary.iterdir()), "failed download was not removed"
        assert not payload.exists()

print(
    "PASS: PATH/user-local/portable Zellij, terminal input, exit status, checksum rejection and cleanup"
)

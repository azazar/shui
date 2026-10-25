"""Offline behavioral check of the launch-only Zellij profile."""

import os
from pathlib import Path
import subprocess
import tempfile

repo = Path(__file__).resolve().parents[1]
config = (repo / "docs/config.example.yml").read_text()
block = config.split("  - name: Zellij\n", 1)[1].split("  - name:", 1)[0]
source = next(
    line.removeprefix("    remote: ")
    for line in block.splitlines()
    if line.startswith("    remote: ")
)

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
    for status in [0, 42]:
        result = subprocess.run(
            ["/bin/sh", "-c", source],
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
        assert not (root / ".local").exists()

print(
    "PASS: Zellij launch, terminal input, user configuration, missing-tool failure and exit status"
)

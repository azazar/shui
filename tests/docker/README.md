# Linux profile integration tests

## Service recipes

```sh
python3 tests/docker/run.py --services --distro debian --distro fedora --timeout 2400
```

This opt-in mode uses privileged containers with private cgroup namespaces, without host networking or host filesystem mounts. Containers and test images are removed on completion or failure. It tests security-update scheduling and execution, real Docker/Podman process execution, journal ingestion/storage limits, archive/cache cleanup, and IPv4/IPv6 firewall traffic through an isolated network namespace. Container storage uses private tmpfs mounts to avoid overlay-on-overlay restrictions; the nested Podman workload disables cgroups because controllers may not be delegated in the test container.

Recursive logging tests run real QEMU guest agents in two nested chroots. A virsh fixture supplies discovery and transport; guest execution and filesystem cleanup use the real agent protocol. This covers nested guest execution and inaccessible-agent reporting, but not full VM boot or libvirt domain discovery. Alpine is excluded because this test mode requires systemd. `--keep` retains resources for debugging.

Optional-runtime wiring is tested with a `runsc` forwarding fixture backed by the installed runc: Docker must load the runtime and execute a container through it while preserving unrelated daemon settings. This verifies registration/reload, not actual gVisor isolation, runq/KVM guests, NVIDIA drivers, or GPU execution. Real optional-runtime packages remain dependent on enabled repositories.

## Profiles and non-service recipes

Run from the repository root with Docker, Python 3, and the pinned Rust toolchain:

```sh
python3 tests/docker/run.py
python3 tests/docker/run.py --distro ubuntu --distro alpine --jobs 2
```

The runner exports fixtures through the app's Rust configuration parser and command renderer, then builds disposable images and runs a real SSH server inside each container. It does not read your SSH keys, mount your home, publish ports, or install packages on the host. The remote account is unprivileged; its test-only sudo access exercises the installers' non-root path.

| Distribution | Image | Package manager |
| --- | --- | --- |
| Debian | `debian:13` | apt-get |
| Ubuntu LTS | `ubuntu:24.04` | apt-get |
| Fedora | `fedora:43` | dnf |
| Rocky Linux | `rockylinux:9` | dnf + EPEL |
| Alpine | `alpine:3.22` | apk |
| Arch Linux | `archlinux:base` | pacman |
| openSUSE Tumbleweed | `opensuse/tumbleweed:latest` | zypper |

Images track distribution updates rather than immutable digests so repository/package changes are exercised. Bootstrap installs test prerequisites: Python, SSH, sudo, certificates, download/archive tools, process tools, diffutils, ncurses, Bash, tmux and btop. Rocky enables EPEL for btop. The harness provisions checksum-verified Zellij 0.45.1; no launch profile installs or downloads software.

## Coverage

| Shipped entry | Assertions |
| --- | --- |
| SSH / Bash | Real interactive shell, target home, explicit port overriding SSH config; no dependency installation |
| Tmux | Create, detach and reconnect to the same session using preinstalled tmux |
| Zellij | Create, detach and reattach using preinstalled Zellij, without forcing a session name; startup tips/release notes are disabled |
| Monitor | Start and quit preinstalled btop without a profile-supplied configuration |
| Configure key-based auth | Generate a real Ed25519 key through the recipe; accept an empty passphrase for the disposable test key; authenticate with it; reuse without duplicate authorized keys; reject overwriting it; verify every expiry duration, expired-key rejection, renewal and removal of expiry |
| Install dotfiles | Use configured SSH authentication; copy dotfiles including hidden files; execute a fixture installer and verify its remote result |
| Configure passwordless sudo | Add the test user to the administrative group, remove fixture privileges, and prove sudo runs as root without a password |
| Configure SSH-agent sudo | Prove authorized-agent authentication and password fallback, reject missing/untrusted agents, preserve timestamp caching and other users' auth policy; verify package-unavailable handling |
| Copy terminal definition | Compile a unique system terminfo fixture for the root client, export/copy it through the recipe, verify it appears in the remote user's private database and temporary files are removed |

No package-manager, SSH, tmux, btop or Zellij executables are mocked. A pseudo-terminal supplies known setup/password prompts and drives interactive applications. Harness downloads retain TLS and checksum verification; package signature checks stay enabled.

Bootstrap checks for `awk`, `cmp`, `tic`, and `infocmp`; openSUSE explicitly installs gawk and ncurses-devel for the terminfo tools. The loopback SSH fixture disables per-source penalties when supported because the expiry tests deliberately attempt rejected authentication. The terminfo check uses the root client's system database and verifies the remote user's private database explicitly, accommodating ncurses builds that ignore root's home directory.

The openSUSE image also creates `/run/tmux` with the same shared-directory permissions normally supplied by systemd-tmpfiles. Containers do not run that service, and the packaged tmux needs the directory when it starts.

The Arch test container adds `PERFMON` and `DAC_READ_SEARCH` to its capability bounding set, matching the file capabilities on its packaged btop. Without these, Linux rejects execution with “Operation not permitted.” The container retains its isolated filesystem and process namespaces; it does not use privileged mode.

**Excluded:** Mosh (UDP/client behavior), MC (VFS client), SFTP (interactive client), Tunnel/SOCKS (client-side forwarding), and Dropbear/PuTTY/Plink (alternative clients). These remain covered by the fast argument/terminal tests where applicable; this suite targets SSH-executable profiles and configuration recipes. The fixture exporter fails when a shipped profile or recipe is added or renamed without an explicit coverage decision.

## Results and troubleshooting

Logs and `report.json` are saved under `target/docker-tests/<run-id>/<distro>/`, keeping earlier results separate. A failed build, failed assertion, missing package, timeout or failed download makes the runner exit nonzero. Unexpected command exit codes include the terminal output tail in the CI error, alongside the complete per-case log. `--timeout` controls the build and container deadlines (1,200 seconds each by default). `--keep` retains this run's named containers and images for inspection; otherwise only those resources are removed. Docker's reusable build cache and downloaded base images remain.

The suite requires network access to the distributions' package repositories and the pinned upstream Zellij release. Network restrictions are failures, not silently skipped tests. Proxy environment variables are passed to builds, containers, SSH and test sudo sessions. Use repeatable `--ca-cert /path/to/trusted-ca.crt` for trusted HTTPS proxy CAs; TLS verification is never disabled.

On GitHub Actions, failure diagnostics also appear as an error annotation on the public run/job page, so the terminal-output tail can be read without downloading the log artifact.

CI exports fixtures once in the Rust job, then runs the seven distributions in parallel and uploads logs on success or failure. These jobs are part of the reusable checks workflow and therefore gate release publication.

To export once and rerun containers without rebuilding Rust tests:

```sh
SHUI_DOCKER_FIXTURES="$PWD/target/docker-fixtures" cargo test --locked export_docker_fixtures -- --ignored
python3 tests/docker/run.py --fixtures target/docker-fixtures --distro debian
```

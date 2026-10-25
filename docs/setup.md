# Setting up a remote

Press **F4** to open remote setup for the selected host, focused on the recipe. Press **Shift+F4** to open **Add new** directly with an empty address. With no selected host, F4 starts in **Add new** mode and keeps a valid address already typed into the search.

Use Left/Right on **Action** to switch between:

- **Configure host**: the selected address is filled in. Check one or more setup recipes and press **Enter** to apply them. Shui returns to the host list with a success or failure message, preserving your filter and profile choices. It does not launch a connection or save a history entry.
- **Add new**: enter `user@host[:port]` (or `user@[IPv6]:port`), and optionally check setup recipes. **Enter** adds and connects using the default launch profile; **Alt+Enter** returns to the menu after the connection ends.

Each action retains its own address draft while you switch. Tab/Shift-Tab or Up/Down moves between fields; Left/Right changes the focused action or dropdown choice. The dialog stays in place when recipes change; short terminals scroll fields to keep the selection visible. **Esc** cancels. All controls can be rebound under `keybindings.dialog`.

All available recipes appear as checkbox rows in one scrollable form. Use Up/Down or Tab/Shift-Tab to navigate, and **Space** to check or uncheck a recipe. Every checked recipe's options appear directly beneath its checkbox, so options for multiple selected recipes stay visible together. Unchecked recipes hide their options but retain edited values. Enter validates all checked recipes before running them in configuration order, stopping at the first failure. Only checked recipes run; an empty selection skips setup in Add new mode.

![Configuring the selected host with a setup recipe](images/configure-remote.png)

![Adding a remote with SSH keypair and dotfiles inputs](images/new-remote.png)

The included setup recipes are:

| Recipe | What it does |
| --- | --- |
| **Configure key-based auth** | Selects an existing keypair or offers generation when none is available, then copies its public key with ssh-copy-id |
| **Install dotfiles** | Uses the configured SSH authentication to upload a dotfiles directory and run its installer |
| **Configure passwordless sudo** | Writes `/etc/sudoers.d/nopasswd` for the distro's administrative group (`sudo` on Debian/Ubuntu, `wheel` on the other supported distros) |
| **Configure SSH-agent sudo** | Installs the packaged PAM module and authorizes the forwarded agent's public keys for sudo |
| **Configure automatic security updates** | Enables daily security-only updates on supported systemd distributions |
| **Install Docker / Podman** | Installs the selected engine and starts Docker when selected |
| **Configure log limits and cleanup** | Configures journal, text, container and VM log limits; cleans archives and optional caches, with optional recursive guest maintenance |
| **Configure firewall** | Configures the distro firewall with optional SSH source restrictions and additional allowed ports |
| **Copy terminal definition** | Exports local `$TERM` and compiles it into remote `~/.terminfo` for terminals unknown to the server |

The dotfiles sample defaults to local `~/src/dotfiles`, installer `install.sh`, and shell `sh`. Change the dialog values or edit the recipe for your own repository. It uses normal SSH configuration and authentication; configure access first if needed. The key-auth recipe remains available when no matching `~/.ssh/id_*` private/public keypairs exist. The key selector reads filenames, not key contents. If no usable pair is found, it offers **Generate new Ed25519 key** instead.

Generation runs only after confirming setup, never while opening or canceling the dialog. It uses `ssh-keygen` to create the standard `~/.ssh/id_ed25519` pair, with the terminal restored for its passphrase prompts. No empty passphrase is forced. Existing private or public files (including symlinks) are not overwritten; restore an incomplete pair or choose another existing pair before retrying. Failure stops setup before `ssh-copy-id` or the connection runs. The generated key remains on disk if later setup fails.

The key-auth recipe offers **Key expiry**: never (default), 8 hours, day, week, month, or year. After copying the key, setup applies OpenSSH's [`expiry-time`](https://man.openbsd.org/sshd.8#expiry-time) option using the server's clock and UTC. Month means 30 days; year means 365 days. **Never** removes an existing expiry for that key. Every matching entry is updated, preserving other restrictions, comments, and unrelated keys. Expiry blocks new authentication; it does not disconnect existing sessions.

The updater expects a regular remote `~/.ssh/authorized_keys` file and an OpenSSH server supporting `expiry-time`. Custom authorized-key locations require adapting the recipe. If the post-copy update fails, setup reports failure, but the copied key remains installed.

Existing configuration files are preserved. To adopt these samples in an existing installation, copy the desired entries under `configuration_scripts` from the [updated example](config.example.yml); keep your custom recipes.

Older bundled setup recipes combine placeholders with `$(...)` command substitution. These templates are now rejected because shell word escaping cannot safely handle nested quoting. Replace the bundled key-auth and dotfiles entries and **Copy terminal definition** with their current versions, including their helper fields and YAML anchors, then run `shui --check-config`. For custom recipes, move command substitution into a literal script field and pass placeholder values as positional arguments. Placeholders inside `${...}` parameter expansions are also rejected; ordinary expressions such as `${port:+-p "$port"}` remain supported beside placeholders.

Setup starts with no recipes checked. Configure mode requires a recipe; adding a remote can skip setup. Authentication prompts run with the terminal restored. Failed setup prevents adding or connecting. In configure mode, failures return to the menu so you can retry. Setup is not repeated on later connections. The dialog itself does not edit SSH config; the selected recipes determine what changes on the local or remote machine.

## Terminal compatibility

Launch profiles handle their own tool dependencies. Select Monitor, Tmux, Mosh, or another profile to check and optionally install what it needs; tool installation is not a host setup recipe.

**Copy terminal definition** addresses errors such as `unknown terminal: xterm-kitty`. It uses the method documented by [Kitty](https://github.com/kovidgoyal/kitty/blob/master/docs/kittens/ssh.rst), generalized to the current `$TERM`. Local `infocmp` and remote `tic` (ncurses tools) are required. The remote compiles a temporary source file, which also works on systems whose tic cannot read `/dev/stdin`, and removes it afterward. Matching definitions in `~/.terminfo` are updated; system definitions are untouched. The recipe is hidden if local ssh, infocmp or TERM is missing. It copies terminal capabilities, not fonts or shell integration.

The terminal-definition recipe uses existing authentication; configure key-based auth first or authenticate normally when prompted.

## Sudo recipes

Both recipes require root access or working sudo. Passwordless sudo grants the administrative group all commands without a password; it does not add the login user to that group. Sudoers fragments are validated with `visudo`, and a failed full validation restores the previous fragment.

SSH-agent sudo uses `ssh -A` during setup and requires keys already loaded in the local agent. It records those public keys in root-owned `/etc/security/ssh-agent-sudo/<user>`, preserves `SSH_AUTH_SOCK` through sudo, and sets `timestamp_timeout=0`. It prepends its authentication rule to the existing `sudo` and, when present, `sudo-i` PAM files, preserving the distro's password/account/session stack. PAM backups use the `.shui-backup` suffix. Re-running replaces this user's allowed agent keys with the currently loaded keys.

The recipe disables sudo's `targetpw`, `rootpw`, and `runaspw` defaults so authentication uses the invoking user's agent keys or password, including on openSUSE.

Forward the agent on subsequent connections to use this authentication (for example, configure `ForwardAgent yes` for the host). This recipe does not grant sudo privileges; existing sudo policy still applies. NOPASSWD rules take precedence when both recipes are enabled.

Package installation supports apt, dnf, zypper, pacman, and apk. Only enabled distro repositories are used: there are no source builds or automatic third-party repository additions. If the package is unavailable (including the tested Alpine, Arch, and Fedora 43 repositories), setup fails before writing PAM or sudoers configuration. A preinstalled module can also be used.

## Automatic security updates

**Configure automatic security updates** enables a daily systemd timer with a randomized delay of up to one hour and catch-up after downtime. It uses security-only unattended-upgrades origins on Debian/Ubuntu, `dnf upgrade --security` on Fedora/RHEL derivatives, and security patches on openSUSE Leap/SLES. Other distributions, including Arch, Alpine and openSUSE Tumbleweed, report unsupported rather than enabling full unattended upgrades. Security classification on RPM systems depends on the enabled repositories' advisory metadata.

The recipe writes `shui-security-updates.service` and `.timer` under `/etc/systemd/system`, plus `/etc/apt/apt.conf.d/52shui-security-updates` on Debian/Ubuntu. It does not trigger an immediate upgrade or automatic reboot. Existing unrelated update schedulers remain enabled.

## Container engines

**Install Docker / Podman** offers an engine dropdown. Packages come from enabled distribution repositories; missing packages cause an installation error. Docker uses `docker.io` on Debian/Ubuntu, `moby-engine` on Fedora/RHEL derivatives where available, and `docker` elsewhere. Podman uses `podman`.

Docker is enabled and started using systemd or Alpine's OpenRC. The recipe verifies the chosen engine with `docker info` or `podman info`. It does not add users to the Docker group or configure rootless Podman. On repositories without Docker/Moby, choose Podman or enable an appropriate repository yourself.

## Log limits and cleanup

**Configure log limits and cleanup** handles detected host logging systems and optionally running libvirt guests. Python 3 and logrotate are installed from the distro repositories when missing.

Inputs:

| Input | Default | Meaning |
| --- | --- | --- |
| Disk/runtime journal size | 512M / 128M | Persistent and volatile journald limits |
| Retention days | 30 | Journal/archive retention and user-cache age threshold |
| Individual log size | 10M | Logrotate default, Docker/Podman defaults and virtlogd log size |
| Rotated files per log | 3 | Logrotate/Docker/virtlogd backup count |
| Cleanup mode | Enforce retention | Remove expired/excess archives and old selected user-cache files, or **Remove all archives and selected caches** to reclaim more space |
| Cache cleanup | None | Package caches, login-user/all-user caches, or both package and selected user caches |
| VM guest scope | Host only | Optionally traverse running guests recursively through QEMU guest-agent execution |
| Maximum guest depth | 2 | 0–5; traversal also has a total budget of 16 guest attempts |

Sizes accept positive bytes or `K`, `M`, `G`, `T` suffixes. The recipe configures:

- **journald**: `/etc/systemd/journald.conf.d/90-shui-limits.conf`, followed by restart, rotation and vacuum. Existing persistent/volatile storage selection is preserved. Active journal files are retained.
- **Text logs**: global defaults in `/etc/logrotate.conf`, preserving service-specific rules and rotation hooks. Explicit per-service overrides remain authoritative. The systemd logrotate timer is enabled, or Alpine's cron service is started. Limits are checked on scheduled rotation, not continuously.
- **Docker**: merges `max-size`/`max-file` into `/etc/docker/daemon.json` for the `json-file` and `local` drivers. Journald-backed Docker logs follow journal limits; other drivers receive a warning. **Docker is not restarted and containers are not recreated**; the recipe reports the necessary follow-up.
- **Podman**: `/etc/containers/containers.conf.d/90-shui-logging.conf` sets bounded `k8s-file` logging for new containers. Existing containers require recreation, and user/workload overrides can take precedence. Podman's file-size limit does not implement the Docker backup-count setting.
- **libvirt/QEMU**: sets `max_size`/`max_backups` in `virtlogd.conf` and re-executes an active virtlogd to reload configuration while preserving VM log descriptors. VMs are not restarted. Traditional numbered/date-suffixed archives under `/var/log`, including libvirt logs, are cleaned according to the chosen mode.

Cleanup preserves active text and engine-owned container logs. The aggressive mode removes archived journals/text logs and selected user-cache contents; it does not delete containers, images, volumes, VM disks or installed packages. Package-cache cleanup uses the package manager's native clean/autoclean commands; its conservative behavior is manager-specific rather than an age threshold.

User-cache cleanup covers `~/.cache`, runs with each user's own privileges, and does not follow symlinks. “All-user” covers root and ordinary local accounts (UID 1000–65533). In guests, the login-user choice means root, because QEMU guest-agent execution runs as root.

Recursive mode uses the system libvirt connection and requires working guest agents with `guest-exec` enabled. Each guest applies the same settings and cleanup choices, then traverses its running guests up to the depth/budget limits. Machine IDs prevent revisiting hosts. Missing/disabled agents, failed commands and incomplete guest results are reported as failures; successful earlier cleanup is not rolled back. Each host execution has a ten-minute command deadline and each guest is waited on for at most three minutes. A timed-out guest command may still be running and is explicitly reported.

## Firewall

**Configure firewall** uses UFW on Debian/Ubuntu and firewalld on Fedora/RHEL derivatives, openSUSE and Arch. It requires systemd. It keeps existing allowed services and adds default-deny inbound filtering; outgoing traffic, established connections and the distro's standard loopback/ICMP handling remain available.

- **SSH allowed IPs/CIDRs**: comma- or whitespace-separated IPv4/IPv6 addresses or networks. Blank allows SSH from any address. The server-side port is obtained from the current SSH connection, including nonstandard ports and SSH aliases.
- **Additional ports**: comma- or whitespace-separated entries such as `80/tcp 443/tcp 53/udp 8000-8010/tcp`. The SSH port must be configured through the SSH source field, not this list.

SSH rules precede ordinary service allowances. UFW marks managed SSH rules with `shui-ssh`; firewalld uses rich-rule priorities -100/-90 and tracks its rules in `/var/lib/shui/firewall-ssh.json`. Firewalld configures the default and currently active zones, then reloads permanent configuration. Existing custom higher-priority rules or policies may still take precedence. Re-running replaces the recipe's SSH source rules; additional port allowances are additive. An already active alternative firewall is rejected rather than enabling both backends.

## Custom inputs

Recipes belong under top-level `configuration_scripts` and share the profile command format. Commented examples are in the shipped config; F9 opens your copy.

`promptString` creates a text field; `pickString` creates a dropdown. IDs are unique within a recipe and contain letters, digits, `_`, or `-`. `description` supplies the label, otherwise the ID is shown. Text fields default to empty; dropdowns default to their first option. The `required` flag rejects blank values.

An optional `pattern` is a [Rust regular expression](https://docs.rs/regex/latest/regex/#syntax) matched against the **entire** value. Optional empty fields skip the pattern. Use `validation_message` to explain the accepted format when validation fails:

```yaml
- id: backups
  type: promptString
  description: Rotated files per log (1-100)
  default: '3'
  required: true
  pattern: '(?:[1-9][0-9]?|100)'
  validation_message: Enter a whole number from 1 to 100
```

Invalid patterns or nonempty defaults/options that do not match are configuration errors. Generated defaults/options must also satisfy the static pattern. Submission focuses the first invalid field and validates all selected recipes before any command runs. Bundled log-size fields accept positive byte counts or uppercase `K/M/G/T` suffixes (for example `512M`, not `512MB`). Firewall lists use spaces or commas; extra ports require `/tcp` or `/udp`. CIDR validity, port-range ordering and SSH-port conflicts are additionally checked on the remote host before firewall changes.

Pass remote-script inputs as positional arguments, as shown in the config comments. Values survive switching recipes within the dialog but are not saved to YAML or history. Fields display plain text and are intended for installation parameters. Launch profiles also support these input declarations in the connection dialog (Ctrl+Enter).

## Generate field defaults or choices

An input's `command` can produce a YAML or JSON object containing `default`, `options`, and/or `description`. Returned fields override static values; omitted fields remain unchanged.

Generators run when the recipe is first checked. Successful results are cached for that dialog. A failure blocks submission and displays an error; uncheck and check the recipe again to retry. Output must be a valid mapping of at most 64 KiB; dropdown choices must be nonempty and contain the default if one is specified. The shipped SSH key selector uses this mechanism.

## Show a recipe only when available

A recipe’s `available_command` controls whether its checkbox appears in the form. The config comments include a custom availability check.

Exit **0** shows the recipe; another status or failure to start hides it. Without a check, the recipe is always available. Checks run once each time the dialog opens. Leaving every recipe unchecked skips setup when adding a remote. Hidden recipes' input generators do not run.

Availability checks and input generators are literal local `sh -c` commands with stdin closed. Use quick, noninteractive commands; menu placeholders are not expanded inside them. Availability output is discarded; generator stdout is parsed as configuration. Setup recipe checks and input generators do not run during config loading or ordinary connections. Launch profiles can also define `available_command`; those checks run once at startup.

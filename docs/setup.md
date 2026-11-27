# Setting up a remote

Press **F4** to open remote setup for the selected host, focused on the recipe. Press **Shift+F4** to open **Add new** directly with an empty address. With no selected host, F4 starts in **Add new** mode and keeps a valid address already typed into the search.

Use Left/Right on **Action** to switch between:

- **Configure host**: the selected address is filled in. Check one or more setup recipes and press **Enter** to apply them. Shui returns to the host list with a success or failure message, preserving your filter and profile choices. It does not launch a connection or save a history entry.
- **Add new**: enter `user@host[:port]` (or `user@[IPv6]:port`), and optionally check setup recipes. **Enter** adds and connects using the default launch profile; **Alt+Enter** returns to the menu after the connection ends.

Each action retains its own address draft while you switch. Tab/Shift-Tab or Up/Down moves between fields; Left/Right changes the focused action or dropdown choice. The dialog stays in place when recipes change; short terminals scroll fields to keep the selection visible. **Esc** cancels. All controls can be rebound under `keybindings.dialog`.

All available recipes appear as checkbox rows in one scrollable form. Use Up/Down or Tab/Shift-Tab to navigate, and **Space** to check or uncheck a recipe. Every checked recipe's options appear directly beneath its checkbox, so options for multiple selected recipes stay visible together. Unchecked recipes hide their options but retain edited values. Enter validates all checked recipes before running them in configuration order, stopping at the first failure. Only checked recipes run; an empty selection skips setup in Add new mode.

After each recipe finishes, Shui shows its exit status and waits for **Enter** locally so you can read the output. This pause also applies to failures, before returning to the menu or reporting the error. It does not change the recipe's success/failure result.

![Configuring the selected host with a setup recipe](images/configure-remote.png)

![Adding a remote with SSH keypair and dotfiles inputs](images/new-remote.png)

The included setup recipes are:

| Recipe | What it does |
| --- | --- |
| **Configure key-based auth** | Selects an existing keypair or generates a new one, then installs its public key over SSH |
| **Install dotfiles** | Uses the configured SSH authentication to upload a dotfiles directory and run its installer |
| **Configure passwordless sudo** | Grants passwordless sudo to the selected user (default) or, explicitly, the distro's administrative group |
| **Configure SSH-agent sudo** | Installs the needed PAM module and authorizes the forwarded agent's public keys |
| **Configure automatic security updates** | Configures native security-only automatic updates where supported |
| **Install Docker / Podman** | Installs the selected engine, enables/starts Docker when selected, and verifies it works |
| **Configure log limits and cleanup** | Configures journal, text, container and VM log limits; cleans archives and optional caches, with optional recursive guest maintenance |
| **Configure firewall** | Configures the distro firewall with optional SSH source restrictions and additional allowed ports |
| **Copy terminal definition** | Exports local `$TERM` and compiles it into remote `~/.terminfo` for terminals unknown to the server |

The dotfiles source defaults to `~/src/dotfiles`; `~` expands to the local home directory on Unix and the user profile on Windows. Relative sources are also resolved under that home. A native tar archive preserves symbolic links instead of uploading their targets outside the selected directory. Both Unix and Windows stream the archive into SSH stdin and remove the local temporary archive afterward; no remote archive or staging directory is created. Windows uses a binary stream, not PowerShell's text pipeline. Tar is required on both ends. The installer defaults to `install.sh`, using `sh`; selecting `bash` installs it remotely if missing. Dependencies of your own installer remain its responsibility. The recipe uses normal SSH authentication; configure access first if needed. The SSH key selector always offers **Generate new Ed25519 key**, alongside every matching existing pair. It reads filenames, not key contents.

Generation runs only after confirming setup, never while opening or canceling the dialog. **New key filename** chooses a filename inside local `~/.ssh` (default `id_ed25519`); choose an unused name when a pair already exists. `ssh-keygen` asks for its passphrase on the restored terminal. No empty passphrase is forced. Existing private or public files (including symlinks) are never overwritten. Failure stops setup before copying the key or connecting; a newly generated key remains if later setup fails.

The installer checks key identity before appending, so an already installed key is not duplicated and existing restrictions are preserved. Authentication with another existing key does not make it skip a new key. Unix remotes use `ssh-keygen` fingerprints from their OpenSSH installation.

The key-auth recipe offers **Key expiry**: never (default), 8 hours, day, week, month, or year. After copying the key, setup applies OpenSSH's [`expiry-time`](https://man.openbsd.org/sshd.8#expiry-time) option using the server's clock and UTC. Month means 30 days; year means 365 days. **Never** removes an existing expiry for that key. Every matching entry is updated, preserving other restrictions, comments, and unrelated keys. Expiry blocks new authentication; it does not disconnect existing sessions.

The updater expects a regular remote `~/.ssh/authorized_keys` file and an OpenSSH server supporting `expiry-time`. Custom authorized-key locations require adapting the recipe. If the post-copy update fails, setup reports failure, but the copied key remains installed.

Existing configuration files are preserved. To adopt these samples in an existing installation, copy the desired entries under `configuration_scripts` from the [updated example](config.example.yml); keep your custom recipes.

Older bundled setup recipes combine placeholders with `$(...)` command substitution. These templates are now rejected because shell word escaping cannot safely handle nested quoting. Replace the bundled key-auth and dotfiles entries and **Copy terminal definition** with their current versions, including their helper fields and YAML anchors, then run `shui --check-config`. For custom recipes, move command substitution into a literal script field and pass placeholder values as positional arguments. Placeholders inside `${...}` parameter expansions are also rejected; ordinary expressions such as `${port:+-p "$port"}` remain supported beside placeholders.

Setup starts with no recipes checked. Configure mode requires a recipe; adding a remote can skip setup. Authentication prompts run with the terminal restored. Failed setup prevents adding or connecting. In configure mode, failures return to the menu so you can retry. Setup is not repeated on later connections. The dialog itself does not edit SSH config; the selected recipes determine what changes on the local or remote machine.

## Terminal compatibility

Launch profiles only run their named tools; install their prerequisites separately. Setup/configuration recipes install dependencies and enable/start services needed for their stated task. They do not run unrelated full-system upgrades or change unrelated application settings. Safety checks, validation, and explicitly selected recipe options still apply.

Dependency installers run only for missing tools, not to refresh already working installations. The logging bootstrap requests only missing components (an existing coreutils-single providing timeout is sufficient). APT refuses official Debian/Ubuntu repositories whose metadata names a different release from the installed `VERSION_CODENAME`; sources are never rewritten. Installation uses `--no-upgrade`, `--no-remove` and `--no-install-recommends`. Necessary dependencies may still be installed or updated within the installed release. DNF disables weak dependencies, and zypper disables recommendations. No full upgrade, autoremove, source build or repository addition is used. Inconsistent repositories must be corrected manually before retrying.

**Copy terminal definition** addresses errors such as `unknown terminal: xterm-kitty`. It uses the method documented by [Kitty](https://github.com/kovidgoyal/kitty/blob/master/docs/kittens/ssh.rst), generalized to the current `$TERM`. Local `infocmp` is required; missing remote `tic` is installed from distro ncurses packages through a terminal-attached preparation connection. The remote then compiles a temporary source file, which also works on systems whose tic cannot read `/dev/stdin`, and removes it afterward. Matching definitions in `~/.terminfo` are updated; system definitions are untouched. The recipe is hidden if local ssh, infocmp or TERM is missing. It copies terminal capabilities, not fonts or shell integration.

The terminal-definition recipe uses existing authentication; configure key-based auth first or authenticate normally when prompted.

## Sudo recipes

Both recipes require root access or working sudo. When connected as root, missing sudo is installed as needed. Passwordless sudo offers a mutually exclusive scope choice: **Selected user** (default; blank user name means the SSH login), or **Administrative group** (`sudo` on Debian/Ubuntu, `wheel` elsewhere). Group scope grants every group member all commands without a password and creates the group if missing; it does not add users to it. User scope does not grant privileges to other users or inspect/create an administrative group. Managed fragments are `/etc/sudoers.d/shui-nopasswd-user-<user>` and `shui-nopasswd-group-<group>`. This is additive: other grants remain unchanged. Sudoers fragments are validated with `visudo`, and a failed full validation restores the previous fragment.

SSH-agent sudo uses `ssh -A` during setup and requires keys already loaded in the local agent. It installs the packaged `pam_ssh_agent_auth` module when missing. It records those public keys in root-owned `/etc/security/ssh-agent-sudo/<user>` and preserves `SSH_AUTH_SOCK` through sudo for that user. It prepends its authentication rule to the existing `sudo` and, when present, `sudo-i` PAM files, preserving the distro's password/account/session stack. PAM backups use the `.shui-backup` suffix. Re-running replaces this user's allowed agent keys with the currently loaded keys. Existing sudo timestamp caching is unchanged.

The per-user `/etc/sudoers.d/ssh-agent-auth-<user>` fragment disables `targetpw`, `rootpw`, and `runaspw` only for this user, so PAM can authenticate their agent or password. It does not change other users' sudo policy.

Forward the agent on subsequent connections to use this authentication (for example, configure `ForwardAgent yes` for the host). This recipe does not grant sudo privileges; existing sudo policy still applies. NOPASSWD rules take precedence when both recipes are enabled.

The recipe uses distro packages only: `libpam-ssh-agent-auth` on Debian/Ubuntu, `pam_ssh_agent_auth` on RPM distributions, or the equivalent package. Availability depends on enabled repositories. If unavailable, setup fails before writing PAM or sudoers configuration; no source builds or third-party repositories are added.

## Automatic security updates

**Configure automatic security updates** configures only distribution-native security-only automation. Debian/Ubuntu use `unattended-upgrades` and the native apt timers. Fedora/RHEL derivatives (including Oracle Linux) use an already installed `dnf5-automatic.timer` or `dnf-automatic-install.timer`, with `upgrade_type = security`. openSUSE Leap/SLES use an already installed YaST automatic online updater and its existing daily/weekly/monthly cron schedule, restricted to security patches. RPM security classification depends on the enabled repositories' advisory metadata. No Shui update service, timer or custom update command is created.

The recipe does not install packages or helpers, invoke immediate package updates, rewrite repositories, autoremove packages, or request a distribution upgrade. Enabling native timers leaves execution to their normal schedule (an overdue native timer may fire). Debian/Ubuntu restrict eligible origins to security pockets of the installed codename and disable all unattended-upgrades autoremove options in `/etc/apt/apt.conf.d/52shui-security-updates`. DNF preserves unrelated automatic-updater settings. Other schedulers remain unchanged; no legacy Shui scheduler migration is performed.

The host and running Docker containers, libvirt LXC containers and QEMU VMs are traversed, including nested guests (three levels, at most 16 direct guests per host). Child commands and discovery have time limits; hitting a limit warns that affected systems need manual updates. Stopped guests are not started. QEMU requires an accessible guest agent and existing `jq`/`base64` on its host. LXC retains its normal security label. No transport dependencies are installed and no security policy is relaxed.

If the OS has no supported native security-only automation (including Arch, Alpine and openSUSE Tumbleweed), the updater/scheduler is missing, or a guest has no accessible shell/agent, the recipe prints a **warning to update that system manually**, not an error, and continues traversal. It does not substitute a full upgrade or a custom scheduler. Actual configuration failures are reported with a failing final status, but other reachable guests are still processed. A timed-out guest-agent command may still be finishing; inspect it before retrying.

**Native auto reboot when required** is an independent, initially unchecked checkbox. Debian/Ubuntu use unattended-upgrades' native `Automatic-Reboot` and reboot-required marker. DNF5 and DNF4 installations exposing the native reboot setting use `when-needed`, never `when-changed`. Unchecking disables reboot in those backends. Where conditional native reboot is unavailable, the recipe warns to reboot manually but still configures security updates. Native reboot timing/user-session policy remains unchanged; a reboot can disconnect sessions. Setup itself never invokes reboot.

## Container engines

**Install Docker / Podman** offers an engine dropdown. Packages come from enabled distribution repositories; missing packages cause an installation error. Docker uses `docker.io` on Debian/Ubuntu, `moby-engine` on Fedora/RHEL derivatives where available, and `docker` elsewhere. Podman uses `podman`.

Docker is enabled and started through systemd or Alpine's OpenRC, then verified with `docker info`. Podman is daemonless and is verified with `podman info`. The recipe does not add users to groups, configure rootless Podman, or run a full-system upgrade. On repositories without Docker/Moby, choose Podman or enable an appropriate repository yourself.

Where Debian splits the Docker client into `docker-cli`, a missing client is installed separately from the daemon. If Podman reports its native default `crun` or `runc` missing, only that runtime is installed; the default is not changed. Working engines/runtimes are not reinstalled.

Independent, initially unchecked Docker-only options install **runsc (gVisor)**, **runq**, and **NVIDIA Container Toolkit** (`nvidia-docker`). Selected runtimes are registered without changing Docker's default runtime or recreating containers; its configuration is validated and reloaded. Only packages from enabled repositories are used—no source builds or repository additions. Unavailable packages cause a clear failure (earlier installation steps are not rolled back). runq requires usable `/dev/kvm`; NVIDIA requires a separately working host GPU driver. These options are rejected when Podman is selected.

## Log limits and cleanup

**Configure log limits and cleanup** uses a Bash script and native logging/filesystem tools on the host and optional running libvirt guests. Inputs are validated before installing dependencies. Only missing tools are installed from distro repositories: Bash, logrotate and timeout, plus GNU findutils when archive or user-cache cleanup is selected. jq is needed only for Docker configuration or active guest traversal/protocol handling. A standalone depth-zero run needs no guest machinery; depth-zero children still return progress to their parent. No embedded Python is used.

Inputs:

| Input | Default | Meaning |
| --- | --- | --- |
| Disk/runtime journal size | 512M / 128M | Persistent and volatile journald limits |
| Retention days | 30 | Journal/archive retention and user-cache age threshold |
| Individual log size | 10M | Logrotate default, Docker/Podman defaults and virtlogd log size |
| Rotated files per log | 3 | Logrotate/Docker/virtlogd backup count |
| Clean expired/excess rotated logs | Checked | Remove only rotated text/VM archives exceeding retention days or backup count; never delete active logs |
| Delete ALL rotated logs and selected caches now | Unchecked | Explicitly ignore age for rotated logs, selected caches and archived journals; requires log cleanup to be checked |
| Clean package caches | Unchecked | Use the package manager's native cache-cleaning behavior |
| Clean login-user ~/.cache | Unchecked | Remove old files unless immediate deletion is explicitly selected |
| Include all local users | Unchecked | Extend user-cache cleanup beyond the login user; requires user-cache cleanup |
| Also clean running VM guests recursively | Unchecked | Apply the same selected options through QEMU guest-agent execution |
| Maximum guest depth | 2 | 0–5; traversal also has a total budget of 16 guest attempts |

Sizes accept positive bytes or `K`, `M`, `G`, `T` suffixes. The recipe configures:

- **journald**: `/etc/systemd/journald.conf.d/90-shui-limits.conf`, followed by restart. Explicit rotation/vacuum runs only when log cleanup is checked. Existing persistent/volatile storage selection is preserved. Native retention applies as journald operates; active journal files are retained.
- **Text logs**: global defaults in `/etc/logrotate.conf`, preserving service-specific rules and rotation hooks. Explicit per-service overrides remain authoritative. The systemd logrotate timer is enabled, or Alpine's cron service is started. An explicit immediate logrotate run occurs only when cleanup is checked. Limits are checked on scheduled rotation, not continuously.
- **Docker**: merges `max-size`/`max-file` into `/etc/docker/daemon.json` for the `json-file` and `local` drivers, without selecting a driver. If the file does not specify one, `docker info` supplies the effective driver; failure leaves the configuration untouched. Journald-backed Docker logs follow journal limits; other drivers receive a warning. **Docker is not restarted and containers are not recreated**; the recipe reports the necessary follow-up.
- **Podman**: `/etc/containers/containers.conf.d/90-shui-logging.conf` sets only `log_size_max`; it does not select a driver or change existing directory permissions. New configuration directories are created readable by ordinary users. Existing journald logging stays journald and follows journal limits. Existing containers require recreation, and user/workload overrides can take precedence. Podman's file-size limit does not implement the Docker backup-count setting.
- **libvirt/QEMU**: sets `max_size`/`max_backups` in `virtlogd.conf` and re-executes an active virtlogd to reload configuration while preserving VM log descriptors. VMs are not restarted. Traditional numbered/date-suffixed archives under `/var/log`, including libvirt logs, are cleaned according to the chosen mode.

Cleanup preserves active text and engine-owned container logs. The aggressive mode removes archived journals/text logs and selected user-cache contents; it does not delete containers, images, volumes, VM disks or installed packages. Package-cache cleanup uses the package manager's native clean/autoclean commands; its conservative behavior is manager-specific rather than an age threshold.

Logrotate's state and libvirt archive paths identify known logs. An unrecognized rotated log requires a per-file **Delete this unknown rotated log? [y/N]** confirmation. No/Enter preserves it. Without a controlling terminal (including QEMU-agent execution), unknown files are preserved with a warning rather than approved automatically. Immediately before deletion, after any confirmation, cleanup checks current open descriptors through `/proc`. Open files and files whose open state cannot be checked are retained regardless of confirmation. Each directory is processed once, including when find splits large file lists into batches. Only candidates already eligible under the selected retention/aggressive scope are considered.

User-cache cleanup covers `~/.cache`, runs with each user's own privileges, and does not follow symlinks. “All-user” covers root and ordinary local accounts (UID 1000–65533). In guests, the login-user choice means root, because QEMU guest-agent execution runs as root.

Recursive mode uses the system libvirt connection and requires working guest agents with `guest-exec` enabled. Each guest applies the same settings and cleanup choices, then traverses its running guests up to the depth/budget limits. Machine IDs prevent revisiting hosts. Missing/disabled agents, failed commands and incomplete guest results are reported as failures; successful earlier cleanup is not rolled back. Each host execution has a ten-minute command deadline and each guest is waited on for at most three minutes. A timed-out guest command may still be running and is explicitly reported.

## Firewall

**Configure firewall** uses a POSIX shell script and installs UFW on Debian/Ubuntu or firewalld on Fedora/RHEL derivatives, openSUSE and Arch, plus iproute2 (named `iproute` on Fedora/RHEL). It requires systemd. Numeric addresses/prefixes are validated without changing routes. Only necessary packages are installed, without a full-system upgrade.

Validation precedes firewall-package installation and rule changes. If the native address validator (`ip`) is missing, only its package is installed first; an existing firewall is not reinstalled. Before changing UFW, setup saves its configuration and active/enabled state. A command failure or handled termination restores them. If restoration itself fails, a warning names the retained backup directory for manual recovery. An uncatchable process kill or machine failure cannot run shell rollback.

**The port whitelist replaces existing incoming allowances, rather than adding ports.** UFW is enabled, then its numbered incoming rules are replaced using the native CLI; outgoing and forwarded rules and their default policies are preserved. Firewalld replaces services, ports, protocols, source-port/forward-port allowances and rich rules in its default and active zones. Incoming traffic not whitelisted is rejected, apart from the separately configured SSH access and standard loopback/established/ICMP handling. Review this before applying it to a host with custom firewall rules.

- **SSH allowed IPs/CIDRs**: comma- or whitespace-separated IPv4/IPv6 addresses or networks. Blank allows SSH from any address. The server-side port is obtained from the current SSH connection, including nonstandard ports and SSH aliases.
- **Port whitelist**: comma- or whitespace-separated entries such as `80/tcp 443/tcp 53/udp 8000-8010/tcp`. Blank permits no additional incoming ports. The SSH port, including a range covering it, is rejected; configure SSH through its separate source field.

UFW labels SSH rules `shui-ssh`; firewalld uses rich-rule priorities -100/-90. Re-running replaces the whitelist and SSH restrictions. Custom firewalld policies or direct rules outside these zones can still take precedence and need separate review. An already active alternative firewall is rejected rather than enabling both backends.

## Custom inputs

Recipes belong under top-level `configuration_scripts` and share the profile command format. Commented examples are in the shipped config; F9 opens your copy.

`promptString` creates a text field; `pickString` creates a dropdown. IDs are unique within a recipe and contain letters, digits, `_`, or `-`. `description` supplies the label, otherwise the ID is shown. Text fields default to empty; dropdowns default to their first option. The `required` flag rejects blank values.

Use `type: checkbox` for independent yes/no options, especially when multiple options may be selected. **Space** toggles the focused checkbox. `default: true` checks it initially; omitted defaults are false. Templates receive the literal string `true` or `false`. Checkboxes work in setup recipes and launch-profile input dialogs and retain their values while navigating. For example: `{id: guests, type: checkbox, description: Clean VM guests, default: false}`. Keep mutually exclusive choices as dropdowns.

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

Invalid patterns or nonempty defaults/options that do not match are configuration errors. Generated defaults/options must also satisfy the static pattern. Submission focuses the first invalid field and validates all selected recipes before any command runs. Bundled log-size fields accept positive byte counts or uppercase `K/M/G/T` suffixes (for example `512M`, not `512MB`). Firewall whitelist entries use `/tcp` or `/udp`, separated by spaces or commas. CIDR validity, port-range ordering and SSH-port conflicts are additionally checked remotely before firewall changes.

Pass remote-script inputs as positional arguments, as shown in the config comments. Values survive switching recipes within the dialog but are not saved to YAML or history. Fields display plain text and are intended for installation parameters. Launch profiles also support these input declarations in the connection dialog (Ctrl+Enter).

## Generate field defaults or choices

An input's `command` can produce a YAML or JSON object containing `default`, `options`, and/or `description`. Returned fields override static values; omitted fields remain unchanged.

Generators run when the recipe is first checked. Successful results are cached for that dialog. A failure blocks submission and displays an error; uncheck and check the recipe again to retry. Output must be a valid mapping of at most 64 KiB; dropdown choices must be nonempty and contain the default if one is specified. The shipped SSH key selector uses this mechanism.

## Show a recipe only when available

A recipe’s `available_command` controls whether its checkbox appears in the form. The config comments include a custom availability check.

Exit **0** shows the recipe; another status or failure to start hides it. Without a check, the recipe is always available. Checks run once each time the dialog opens. Leaving every recipe unchecked skips setup when adding a remote. Hidden recipes' input generators do not run.

Availability checks and input generators are literal local `sh -c` commands with stdin closed. Use quick, noninteractive commands; menu placeholders are not expanded inside them. Availability output is discarded; generator stdout is parsed as configuration. Setup recipe checks and input generators do not run during config loading or ordinary connections. Launch profiles can also define `available_command`; those checks run once at startup.

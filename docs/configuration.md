# Launch profiles

Edit **`$XDG_CONFIG_HOME/shui.yml`** using the config editor (F9 by default) for validated live reload, or restart after editing externally. The file is created from shared defaults plus the build’s platform additions on the first interactive launch, creating missing directories. Existing configs are preserved. If `XDG_CONFIG_HOME` is unset, empty, or relative, Shui uses `~/.config/shui.yml`.

Run `shui --config-path` to print the active path. Use `shui --check-config` (or `shui --check-config path/to/config.yml`) to validate YAML, templates and keybindings without running availability checks, generators or other commands. Validation never creates a configuration file.

`username_display` controls the host list: `auto` (default) shows usernames only to distinguish accounts for the same host and port, `always` shows known usernames, and `never` hides them. Commands omit an explicit username only when it matches a proven static SSH `User` setting; different or uncertain accounts remain explicit.

Configuration examples and editing guidance live in the comments of the shipped config, available through F9.

Each profile needs a unique `name` and a `command`; `default` names one of them. Commands run through local `sh -c`, and additional string fields can hold reusable commands or scripts. **Placeholders are shell-escaped automatically—leave them unquoted.**

## Included profiles

Profiles appear only when their local clients are available. Alongside SSH, Mosh, MC, Bash and Zellij, the defaults include:

| Profile | Behavior and requirements |
| --- | --- |
| Dropbear | Runs `dbclient` with an optional `-p` port. Requires local Dropbear client. |
| PuTTY | Opens `putty.exe -ssh` on Windows, with an optional `-P` port. Requires PuTTY on PATH. |
| Plink | Runs `plink.exe -ssh -t` in the current Windows terminal. Requires Plink on PATH. |
| Tmux | Attaches to a remote session or creates one, without forcing a session name. Requires remote tmux. Detach with tmux's Ctrl+B, then D. |
| SFTP | Interactive OpenSSH file transfers; type `help` for commands and `bye` to exit. Requires local sftp and a remote SFTP subsystem. |
| Monitor | Runs remote btop with its normal user configuration. Requires remote btop. |
| Tunnel | Opens a dialog for `local port:destination host:port`, default `8080:localhost:80`. The destination is resolved/reached from the SSH server. |
| SOCKS | Opens a dialog for the local proxy port, default `1080`. Configure your client to use SOCKS5 at `127.0.0.1:1080`, with proxy-side DNS when desired. |

Profiles launch the named client or remote application without installing software, downloading executables, or replacing configuration files. Monitor uses your btop configuration; Zellij uses your configuration without forcing a session name, with startup tips and release notes disabled through command-line options.

Both forwarding profiles bind only to `127.0.0.1`, remain in the foreground, and stop with Ctrl+C. They request no remote shell, fail if the listening socket cannot be created, and use SSH keepalives to detect a lost connection. A successful listener does not guarantee the destination service is reachable; the SSH server must allow forwarding. Use Alt+Enter in the dialog to return to the menu when the tunnel stops. Explicit SSH ports and alias settings work as in the other profiles.

Existing configs are not migrated. Copy the desired entries from the [shared defaults](config.example.yml), [Windows overrides](config.windows.yml), or [optional examples](config.optional.yml), choosing unused shortcuts if yours differ.

These presets follow the upstream [tmux manual](https://github.com/tmux/tmux/blob/master/tmux.1), [OpenSSH ssh manual](https://github.com/openssh/openssh-portable/blob/master/ssh.1), [SFTP manual](https://github.com/openssh/openssh-portable/blob/master/sftp.1), and the [btop project](https://github.com/aristocratos/btop).

## Profile shortcuts

Set a profile’s `key` field to connect the selected host with it (or open its input dialog). Hold its modifier group to reveal available shortcuts in the action bar, or open the Keys help topic. They apply only to the host list, preserve the filter while selecting the target, and do nothing when no host matches. Ordinary launches are remembered in history; dialog launches are temporary. Hidden profiles have no active shortcut.

Use **Ctrl+letter**, **Alt+letter**, or **Ctrl+Alt+letter**, with an ASCII letter A–Z. Names are case-insensitive; `Alt+M` does not require Shift. Plain letters, Shift-only bindings, special keys, and duplicate bindings are rejected. Duplicate checks include unavailable profiles so installing a tool cannot introduce a conflict.

Ctrl+H, Ctrl+I, Ctrl+J, Ctrl+M, Ctrl+S, and Ctrl+Z remain reserved for profile shortcuts because of terminal aliases and flow/suspend behavior. Other profile keys must not overlap configured menu actions. Ctrl+C can be reused after removing it from `keybindings.menu.quit`. App actions themselves are configurable; see [keybindings](keybindings.md).

The Keys help topic lists active profile shortcuts. Setup recipes do not accept `key`.

## Missing tools

Default profiles check local clients before appearing in the menu. Install local and remote prerequisites separately. A missing remote executable produces the remote shell's normal error; there is no installation prompt or package-manager invocation.

SSH and forwarding profiles require local SSH; MC requires local MC and SSH; SFTP requires local SFTP. Bash, Tmux, Zellij and Monitor need their named application on the remote PATH. Mosh requires local `mosh`/`ssh` and remote `mosh-server`; Shui does not open an additional preparation connection or configure UDP access.

`prepare_command` is an optional local shell command run with the terminal restored before host setup, connection launch, or history writes. It supports target placeholders and named fields, but not dialog inputs. Exit 0 continues; cancellation, failure or failure to start returns to the menu without launching or recording a connection. It runs only for launch profiles, including shortcuts and one-off connections; it does not run when opening the menu or configuring a host.

No shipped profile uses `prepare_command`; it remains available for explicit custom workflows. Customization examples live in [optional examples](config.optional.yml).

## Profile availability

An optional `available_command` checks whether a launch profile can be used. It runs once per profile when the configuration is loaded for an interactive session, before the TUI opens. Exit **0** keeps the profile; a nonzero status or failure to start hides it. Profiles without a check remain available. Identical checks are shared within each load/reload.

Checks time out after five seconds. Input generators have the same deadline and a 64 KiB limit for each output stream, enforced while reading. On Unix, checks and generators run without a controlling terminal, and their process group is stopped on completion, timeout, or output-limit failure. Windows uses a job object to stop the shell and its descendants on the same paths. Interactive work belongs in preparation or connection commands.

Checks are literal local `sh -c` commands with stdin closed and output discarded. Use quick, noninteractive checks; placeholders are not expanded and no remote connection is made automatically. Checks do not run for `--dump`, `--help`, or `--version`.

If the configured default is unavailable, the first available profile becomes the session default. History restoration and F8 only select available profiles. If none remain, Shui reports an error before opening the TUI. Filtering does not rewrite your config; restart after installing a missing tool.

Every shipped launch profile has an `available_command` for its local clients: SSH-based profiles require `ssh`, Mosh requires `ssh` and `mosh`, MC requires `ssh` and `mc`, SFTP requires `sftp`, Dropbear requires `dbclient`, and PuTTY/Plink require their respective Windows executables. Existing config files are preserved; copy the simplified profiles from the [shared defaults](config.example.yml) and [Windows overrides](config.windows.yml).

## Command execution

`command`, `available_command`, and `prepare_command` use the profile’s `shell`: `sh` (default), `direct`, or `powershell`. `prepare_shell` overrides it for preparation only. Input generators have their own `shell` field, defaulting to `sh`.

With `direct`, Shui splits the command into an executable and arguments using shell-style quoting, then launches it without a shell. Pipes, variable expansion, and shell operators are not evaluated. Optional `port_args` are prepended only when the target has an explicit port; `%p` in these arguments is replaced with that port.

PowerShell commands run without loading user profiles. Top-level placeholders become literal PowerShell strings; nested named fields keep POSIX quoting for remote scripts. Put local PowerShell scripts in literal fields without template substitutions, passing values as parameters. Background checks and generators retain their timeout and process cleanup protections for every execution mode.

## Placeholders and ports

| Placeholder | Value |
| --- | --- |
| `%h` | Host or alias, including username when present |
| `%p` | Explicit port, or an empty string |
| `%t` | Full target, including an explicit `:port` when present |
| `%{field}` | Another string field in the same profile or recipe |
| `%{input:id}` | A setup recipe's dialog input |
| `%%` | A literal percent in a command template |

**Leave placeholders unquoted.** Do not combine placeholders with here-documents, command substitution, arithmetic expansion or legacy backticks; those contexts cannot safely use shell word quoting and are rejected. Placeholders inside `${...}` parameter expansions are also rejected; placeholder-free optional-port expressions remain valid. Pass values as positional arguments to a literal script field instead. Shui shell-escapes their values. Input values remain literal even when they contain shell syntax or placeholder text.

An absent explicit port leaves SSH alias settings in effect. The shipped templates handle both explicit ports and aliases.

## Remote scripts

A reusable `remote` field preserves quoting through both the local and remote shell; the config comments show the structure.

Each named-field substitution adds a shell-quoting layer. Fields without `%{...}` references remain literal, including shell expressions in a script. Missing references, reference cycles, and quoted placeholders are rejected when configuration is loaded.

## Menu behavior and editing

`connect` hands off to a connection; `connect_and_return` reopens the menu afterward (Enter and Alt+Enter by default). Configure either action under `keybindings.menu` and `keybindings.dialog`. **F9** immediately edits this file using `$VISUAL` / `$EDITOR`, falling back to `sensible-editor`, `editor`, then `vi` on Unix, or Notepad on Windows. If editing or validation fails, the recovery screen offers a retry and F1 for the format reference. Saving reloads only valid configurations. Checks include YAML structure, required fields, templates, input definitions, unique names and shortcut conflicts. This validates the configuration format, not arbitrary shell script behavior.

## Connection inputs

Launch profiles may declare `inputs` using the same `promptString`, `pickString` and generator format as configuration scripts. Use `%{input:id}` to substitute a shell-escaped value. **Ctrl+Enter** opens a one-off connection dialog; Enter and profile shortcuts also open it when a profile declares inputs. Required values are validated before launch. Values are visible and are never saved to YAML or history. See [connection dialog reference](connection.md).

## Windows

Windows defaults launch connection clients directly, with built-in Windows PowerShell for setup and input generators. No Git installation or local POSIX shell is required. Install the desired client and make it available on PATH. Existing custom configs using POSIX commands still need their chosen shell; use `shui --default-config` to view this build’s complete defaults and copy desired profiles into your config to remove that dependency. The [Windows overrides](config.windows.yml) are build-time fragments, not a standalone user config. Shui reads the native `USERPROFILE` and `USERNAME`, ignores a conflicting POSIX `HOME`, and defaults to Notepad for config editing. `VISUAL` / `EDITOR` use Windows command-line quoting (quote executable paths containing spaces). Configuration remains under `XDG_CONFIG_HOME/shui.yml`, falling back to `%USERPROFILE%/.config/shui.yml`; discovery reads `%USERPROFILE%/.ssh/config`. Read-only CLI commands do not require a shell. For WSL, install a Linux build inside WSL instead.

## Upgrading

If upgrading from an older configuration path, move your existing configuration to `$XDG_CONFIG_HOME/shui.yml` (default: `~/.config/shui.yml`) before the first launch to preserve custom settings. Older paths are not loaded or migrated automatically. When upgrading from the previous project name, update shell aliases and scripts to invoke `shui`.

## Default config parts

Builds merge [shared defaults](config.example.yml) with [Windows overrides](config.windows.yml) for Windows targets. Linux and macOS defaults omit PuTTY and Plink. Each binary embeds one complete config; `shui --default-config` prints it without changing files. There are no runtime includes. Existing user configs are preserved.

[Optional examples](config.optional.yml) contain editable keybindings, custom commands, setup recipes, and generated inputs as active YAML, with [native Windows variants](config.optional.windows.yml). Tests merge these examples with both platform variants and validate their commands and bindings. Builds append these tested examples as comments in new user configs, so they are available through F9 without being enabled. Uncomment desired entries and merge them into the existing sections.

Build-time merging combines mappings recursively and matches `profiles` and `configuration_scripts` by `name`, preserving unspecified fields and entry order. Other lists, including keybindings and inputs, are replaced. A `null` override removes an inherited field. To customize a live config, copy the desired entries or fields into it; the live file does not use this build-time merge mechanism.

## Alternative SSH clients

**Dropbear** (Alt+D) uses [dbclient](https://github.com/mkj/dropbear/blob/master/manpages/dbclient.1). **PuTTY** (Alt+P) opens a [PuTTY window](https://the.earth.li/~sgtatham/putty/latest/htmldoc/Chapter3.html#using-cmdline) on Windows. **Plink** (Alt+L) uses [Plink](https://the.earth.li/~sgtatham/putty/latest/htmldoc/Chapter7.html) in the current Windows terminal with a remote PTY. All three accept explicit `user@host:port` targets; each client's normal host-key verification remains enabled.

These clients do not read OpenSSH's `~/.ssh/config`. Use a real hostname/address, or a PuTTY saved session, and configure keys and proxies in the chosen client. OpenSSH aliases, `User`, `IdentityFile`, and `ProxyJump` settings are not translated. Explicit usernames are retained in these clients' commands, even when they match an OpenSSH `User` directive. PuTTY opens its own window; Plink stays in the current terminal. PuTTY is preferable for full-screen remote applications; Plink’s terminal behavior depends on the Windows console.

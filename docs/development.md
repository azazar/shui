# Development

## Build from source

Install [Rust via rustup](https://rustup.rs/), then:

```sh
git clone https://github.com/azazar/shui.git
cd shui
cargo build --locked --release
./target/release/shui
```

The repository pins its Rust toolchain. Check out the desired release tag before building a specific version. On Linux x86-64, add `--target x86_64-unknown-linux-musl` for a static build; the binary will be in `target/x86_64-unknown-linux-musl/release/`.

For development builds, use the **Artifacts** section of a successful [Binaries workflow](https://github.com/azazar/shui/actions/workflows/binaries.yml) run. Downloading Actions artifacts requires signing in to GitHub. Each artifact ZIP contains a platform archive and its `.sha256` checksum file—extract that outer ZIP first.

## Tests

```sh
python3 -m pip install pyte==0.8.2
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 tests/zellij.py
python3 tests/install.py
cargo build --locked --release --target x86_64-unknown-linux-musl
python3 tests/cli.py target/x86_64-unknown-linux-musl/release/shui
python3 tests/terminal.py target/x86_64-unknown-linux-musl/release/shui
```

The shared Rust action suite (`cargo test --locked action_tests`) enumerates the binding registry and runs every action, alias and an alternative binding through the production menu, dialog, help and editor loops. It checks selections, generated commands, text edits, focus, scrolling, modifier hints and editor reloads using one in-memory terminal harness. New actions without an outcome scenario fail the suite. It also covers shipped profile shortcuts, key releases, repeats and dialog validation. Only terminal I/O and external editor execution are substituted; commands are inspected without contacting hosts.

The terminal suite checks rendered screens after real keyboard, modifier, focus and resize events. It compares dialog anchors across recipes and profiles, scroll positions, visible selections and shortcut groups at several terminal sizes. Use `python3 tests/terminal.py PATH_TO_SHUI --interaction-only` for this focused pass. When auditing UI changes, check the entire screen before and after an interaction; checking that individual labels occur in terminal output misses stale hints, overlapping content and layout movement.

The Zellij test reads its launch command directly from the shipped config and checks terminal input, exit status, and absence of installs/settings changes. The fast Python tests above use temporary fixtures and fake connection tools; they do not connect to remote hosts. To refresh screenshots, install Pillow and pyte, build the static binary, then run `python3 scripts/screenshot.py`, `python3 scripts/screenshot.py --dialog`, `python3 scripts/screenshot.py --configure`, `python3 scripts/screenshot.py --connection`, `python3 scripts/screenshot.py --shortcuts`, `python3 scripts/screenshot.py --shift-shortcuts`, and `python3 scripts/screenshot.py --help-viewer`. Use `python3 scripts/screenshot.py --search` to capture hint-text search. To capture btop, put it on PATH and run `python3 scripts/screenshot.py --monitor`; the real Monitor command runs locally through a fake SSH command. These use fontconfig and DejaVu Sans Mono for Powerline; `SHUI_SCREENSHOT_FONT` can point to another compatible TTF.

The [Docker integration suite](https://github.com/azazar/shui/blob/master/tests/docker/README.md) runs shipped SSH-executable profiles and setup recipes against Debian, Ubuntu, Fedora, Rocky Linux, Alpine, Arch and openSUSE. Run `python3 tests/docker/run.py` locally, or select distributions with `--distro`. The harness provisions application dependencies; profiles themselves only launch them. It exercises real SSH, multiplexers, monitoring, key setup, dotfiles and terminfo inside disposable containers; opt-in service tests cover server recipes. Mosh, MC, SFTP and client forwarding are explicitly excluded. Logs are retained under `target/docker-tests/`; the CI matrix gates releases.

The [Binaries workflow](https://github.com/azazar/shui/blob/master/.github/workflows/binaries.yml) runs on pull requests, branch pushes, version tags, and manual dispatch. It runs the [Linux checks](https://github.com/azazar/shui/blob/master/.github/workflows/ci.yml), builds the platform matrix, runs native CLI and unit tests, verifies static Linux linkage, and uploads packaged artifacts. Both Intel and Apple Silicon macOS runners also run Clippy, the Zellij tests, and the full PTY suite using the same test sources as Linux. Linux-only desktop-terminal checks remain Linux-only. macOS testing uses GitHub-hosted hardware; no local Darling setup is required. All three Linux targets also execute the shared action, binding, modifier, help and dialog-rendering tests through `cross test` (QEMU for ARM). Native Windows and macOS unit tests and the Wine job run the same tests. The in-memory suite tests decoded keyboard events and application outcomes; the PTY suite additionally checks Unix terminal byte decoding, rendering and process handoff.

Before publishing, update the version in `Cargo.toml` and `Cargo.lock`, update the README download URLs and the versions in `scripts/install.sh` and `scripts/install.ps1`, and refresh screenshots if the UI changed. Commit those changes and push a matching `v*` tag to publish. After all builds and checks pass, the workflow uploads the archives and `SHA256SUMS` to a draft, then publishes it. Uploading before publication supports immutable releases. Tags containing a hyphen, such as `v0.1.0-rc.1`, are marked as prereleases. To publish an existing draft, run the **Publish existing release** workflow with its tag; it verifies all seven archives and their checksums first. Windows builds statically link the MSVC runtime; macOS archives are not signed or notarized.

The shared installer tests use local release fixtures and exercise the shell installer on Linux/macOS and PowerShell on Windows. They check platform selection, installation, checksums, failure preservation and temporary cleanup without network access. The one-line installers are served from `scripts/` by GitHub Pages.

## Documentation and help

The user guide, launch profiles, and setup recipes in `docs/` are also the built-in help source, selected by the active screen: Guide, Profiles and Bindings for the host list, Setup for the remote setup dialog, Connection for one-off settings, and Profiles/Bindings/Setup for the configuration editor. Each context also adds a generated Keys topic describing the currently configured bindings. `src/help.rs` embeds them with `include_str!`; rebuilding updates F1 help. Installation instructions in the README, development, and the documentation index remain repository-only content. Add new pages to the documentation index, and include them in help only when useful while running the app. Release archives include the Markdown files as well as the compiled help.

Shared defaults live in `docs/config.example.yml`, Windows overrides in `docs/config.windows.yml`, and optional examples in `docs/config.optional.yml` with native Windows variants in `docs/config.optional.windows.yml`, all as active YAML. `scripts/build.rs` uses the shared merger in `src/config_defaults.rs` to embed the target-specific config; tests compose both platforms with optional examples. The generated user config includes the optional source as comments; a round-trip test ensures it exactly matches the tested YAML and stays inactive. Keep Markdown help focused on usage and field semantics so examples have one maintained home. Screenshots live in `docs/images/`; the screenshot generator writes there, and release archives preserve the same documentation and config paths.

## Windows tests under Wine

```sh
docker build -t shui-wine-tests -f tests/wine.Dockerfile .
docker run --rm --mount "type=bind,src=$PWD,dst=/src,readonly" shui-wine-tests
```

The Wine job also gates CI. It cross-compiles the Windows GNU binary, runs its Rust tests with a checksum-pinned BusyBox Windows shell, then runs the shared CLI suite against Windows paths, home lookup, SSH includes, configuration validation and redirected streams. It exercises Windows pipe capture and job-object cleanup, including timeouts and surviving descendants. The disposable container owns the Wine prefix and build output; the checkout is read-only. Native Windows CI tests the MSVC builds, including a subprocess with Git and POSIX shells removed from PATH. Native executable fixtures record arguments and stdin for connection and PowerShell setup tests. Wine runs the direct-execution portion; built-in PowerShell is covered on Windows runners. Git Bash remains installed only for explicitly POSIX test fixtures. Wine does not verify Windows Terminal rendering, native keyboard input, or macOS behavior.

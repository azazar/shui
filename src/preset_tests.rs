//! Execute shipped shell recipes with isolated PATHs; never contact a server.
use crate::{
    Target,
    launch::{Config, Profile},
};
use std::{
    fs,
    io::Write,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    process::{Command, Output, Stdio},
};

struct Sandbox(PathBuf);
impl Sandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("shui-presets-{name}-{}", std::process::id()));
        fs::create_dir_all(root.join("bin")).unwrap();
        let sandbox = Self(root);
        sandbox.link("sh");
        sandbox
    }
    fn stub(&self, name: &str, body: &str) {
        let path = self.0.join("bin").join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn link(&self, name: &str) {
        let found = Command::new("sh")
            .args(["-c", &format!("command -v {name}")])
            .output()
            .unwrap();
        assert!(found.status.success());
        if name == "awk" {
            let program = String::from_utf8(found.stdout).unwrap();
            self.stub(
                name,
                &format!(
                    r#"{} "$@"
status=$?
if [ "$status" -ne 0 ]; then printf '\nawk argument: <%s>\n' "$@" >&2; fi
exit "$status""#,
                    shell_words::quote(program.trim())
                ),
            );
            return;
        }
        symlink(
            String::from_utf8(found.stdout).unwrap().trim(),
            self.0.join("bin").join(name),
        )
        .unwrap();
    }
    fn run(&self, profile: &Profile, port: Option<&str>, input: Option<(&str, &str)>) -> Output {
        self.run_input(profile, port, input, b"")
    }
    fn run_input(
        &self,
        profile: &Profile,
        port: Option<&str>,
        input: Option<(&str, &str)>,
        stdin: &[u8],
    ) -> Output {
        let mut values = profile.input_defaults();
        if let Some((id, value)) = input {
            values.insert(id.into(), value.into());
        }
        let (_, args) = profile
            .command_with_inputs(
                &Target {
                    host: "user@host".into(),
                    port: port.map(str::to_owned),
                },
                &values,
            )
            .unwrap();
        let mut child = Command::new("/bin/sh")
            .current_dir(&self.0)
            .args(args)
            .env("PATH", self.0.join("bin"))
            .env("HOME", &self.0)
            .env("TMPDIR", &self.0)
            .env("TERM", "test-terminal")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin).unwrap();
        child.wait_with_output().unwrap()
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn profile(name: &str) -> Profile {
    let config = Config::parse(&crate::config_defaults::defaults(false, true).unwrap()).unwrap();
    config
        .profiles
        .into_iter()
        .chain(config.configuration_scripts)
        .find(|p| p.name == name)
        .unwrap()
}
fn stdout(out: Output) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn optional_generators_escape_values_and_propagate_failures() {
    let env = Sandbox::new("optional-generators");
    env.link("sed");
    env.stub(
        "git",
        "[ \"$*\" = 'config --global user.email' ] || exit 99; printf \"o'hara@example.org\\n\"",
    );
    let generated = profile("Generated inputs");
    let run = |index: usize| {
        Command::new("/bin/sh")
            .args(["-c", generated.inputs[index].command.as_ref().unwrap()])
            .env("PATH", env.0.join("bin"))
            .env("HOME", &env.0)
            .output()
            .unwrap()
    };
    let email: serde_yaml_ng::Value = serde_yaml_ng::from_str(&stdout(run(0))).unwrap();
    assert_eq!(email["default"].as_str(), Some("o'hara@example.org"));
    let mode: serde_yaml_ng::Value = serde_yaml_ng::from_str(&stdout(run(1))).unwrap();
    assert_eq!(mode["default"].as_str(), Some("full"));
    assert_eq!(mode["options"].as_sequence().unwrap().len(), 2);
    env.stub("git", "exit 42");
    let output = run(0);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn optional_commands_execute_the_documented_scripts() {
    let env = Sandbox::new("optional-commands");
    env.stub("work-session.sh", "printf 'work\\n'");
    env.stub(
        "bash",
        "if [ \"$1\" = -l ]; then printf 'login\\n'; else exec sh \"$@\"; fi",
    );
    env.stub("install-tools.sh", "printf '<%s>\\n' \"$@\"");
    let mut work = profile("Work");
    work.command = "sh -c %{remote}".into();
    assert_eq!(stdout(env.run(&work, None, None)), "work\nlogin\n");
    for name in ["Install tools", "Generated inputs"] {
        let mut recipe = profile(name);
        recipe.command = "sh -c %{remote}".into();
        for input in &mut recipe.inputs {
            if input.command.is_some() && input.kind == crate::launch::InputType::PickString {
                *input = input.resolve().unwrap();
            }
        }
        let email = "o'hara; $(touch injected)";
        let mode = if name == "Install tools" {
            "minimal"
        } else {
            "full"
        };
        assert_eq!(
            stdout(env.run(&recipe, None, Some(("email", email)))),
            format!("<--email>\n<{email}>\n<--mode>\n<{mode}>\n")
        );
        assert!(!env.0.join("injected").exists());
    }
    env.stub("ssh", "printf '<%s>\\n' \"$@\"");
    assert_eq!(
        stdout(env.run(&profile("Remote command"), None, None)),
        "<user@host>\n<uptime>\n"
    );
    let custom = profile("Install custom tools");
    let check = |profile: &Profile| {
        Command::new("/bin/sh")
            .args(["-c", profile.available_command.as_ref().unwrap()])
            .env("PATH", env.0.join("bin"))
            .output()
            .unwrap()
            .status
            .success()
    };
    assert!(!check(&custom));
    env.stub("my-setup-tool", "printf '<%s>\\n' \"$@\"");
    assert!(check(&custom));
    assert_eq!(stdout(env.run(&custom, None, None)), "<user@host>\n");
}

#[test]
fn windows_key_installation_preserves_existing_keys_and_is_idempotent() {
    let env = Sandbox::new("windows-key-install");
    for tool in ["mkdir", "touch", "chmod", "tr", "awk"] {
        env.link(tool);
    }
    let config = Config::parse(&crate::config_defaults::defaults(true, false).unwrap()).unwrap();
    let mut profile = config.configuration_scripts[0].clone();
    profile.shell = crate::command::Shell::Sh;
    profile.command = "sh -c %{install_key}".into();
    profile.inputs.clear();
    fs::create_dir_all(env.0.join(".ssh")).unwrap();
    let file = env.0.join(".ssh/authorized_keys");
    let previous = "restrict ssh-ed25519 BBBB previous";
    fs::write(&file, previous).unwrap(); // Also exercise a file without a final newline.
    for _ in 0..2 {
        stdout(env.run_input(&profile, None, None, b"ssh-ed25519 AAAA new key\r\n"));
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            format!("{previous}\nssh-ed25519 AAAA new key\n")
        );
    }
    fs::rename(&file, env.0.join("other-keys")).unwrap();
    symlink(env.0.join("other-keys"), &file).unwrap();
    assert!(
        !env.run_input(&profile, None, None, b"ssh-ed25519 CCCC rejected\n")
            .status
            .success()
    );
    assert!(
        !fs::read_to_string(env.0.join("other-keys"))
            .unwrap()
            .contains("CCCC")
    );
}

#[test]
fn shipped_profiles_check_every_local_dependency_without_running_it() {
    let env = Sandbox::new("availability");
    for (name, tools) in [
        ("SSH", vec!["ssh"]),
        ("Mosh", vec!["ssh", "mosh"]),
        ("MC", vec!["ssh", "mc"]),
        ("Bash", vec!["ssh"]),
        ("Zellij", vec!["ssh"]),
        ("Tmux", vec!["ssh"]),
        ("SFTP", vec!["sftp"]),
        ("Monitor", vec!["ssh"]),
        ("Tunnel", vec!["ssh"]),
        ("SOCKS", vec!["ssh"]),
        ("Dropbear", vec!["dbclient"]),
    ] {
        let check = profile(name).available_command.unwrap();
        let available = |os| {
            Command::new("/bin/sh")
                .args(["-c", &check])
                .env("PATH", env.0.join("bin"))
                .env("OS", os)
                .current_dir(&env.0)
                .output()
                .unwrap()
                .status
                .success()
        };
        assert!(
            !available("Windows_NT"),
            "{name} visible without its client"
        );
        for tool in &tools {
            env.stub(tool, "echo executed > unexpected; exit 99");
        }
        assert!(available("Windows_NT"), "{name} hidden with its clients");
        assert_eq!(available("Linux"), !matches!(name, "PuTTY" | "Plink"));
        for missing in &tools {
            fs::remove_file(env.0.join("bin").join(missing)).unwrap();
            assert!(!available("Windows_NT"), "{name} did not check {missing}");
            env.stub(missing, "echo executed > unexpected; exit 99");
        }
        assert!(!env.0.join("unexpected").exists());
        for tool in tools {
            fs::remove_file(env.0.join("bin").join(tool)).unwrap();
        }
    }
}

#[test]
fn shipped_transports_preserve_ports_and_tunnel_arguments() {
    let env = Sandbox::new("transports");
    for name in ["ssh", "sftp"] {
        env.stub(name, "printf '<%s>\\n' \"$@\"");
    }
    for (port, ssh_port, sftp_port) in [
        (None, "", ""),
        (Some("2222"), "<-p>\n<2222>\n", "<-P>\n<2222>\n"),
    ] {
        assert_eq!(
            stdout(env.run(&profile("SFTP"), port, None)),
            format!("{sftp_port}<-->\n<user@host>\n")
        );
        let tmux = stdout(env.run(&profile("Tmux"), port, None));
        assert!(tmux.starts_with(&format!(
            "{ssh_port}<-tt>\n<-->\n<user@host>\n<tmux attach-session"
        )));
        assert!(tmux.contains("exec tmux new-session"));
        for (name, flag, value) in [("Tunnel", "L", "8080:localhost:80"), ("SOCKS", "D", "1080")] {
            let out = stdout(env.run(&profile(name), port, None));
            assert!(out.contains(&format!("{ssh_port}<-N>\n<-T>")), "{out}");
            assert!(out.contains("<ExitOnForwardFailure=yes>"));
            assert!(
                out.ends_with(&format!(
                    "<-{flag}>\n<127.0.0.1:{value}>\n<-->\n<user@host>\n"
                )),
                "{out}"
            );
        }
    }
    let value = "8080:$(touch injected);'host:80";
    let mut tunnel = profile("Tunnel");
    assert!(tunnel.inputs[0].validate_value(value).is_err());
    // Custom recipes can omit validation; argument quoting must still be safe.
    tunnel.inputs[0].pattern = None;
    let out = stdout(env.run(&tunnel, None, Some(("forward", value))));
    assert!(out.contains(&format!("<127.0.0.1:{value}>")));
    assert!(!env.0.join("injected").exists());
}

#[test]
fn monitor_uses_existing_configuration_and_propagates_failure() {
    let env = Sandbox::new("monitor");
    for tool in ["cat", "mktemp", "mkdir", "rm"] {
        env.link(tool);
    }
    let mut p = profile("Monitor");
    p.command = "sh -c %{remote}".into();
    for tool in ["top", "htop"] {
        env.stub(tool, "echo wrong-monitor");
    }
    let missing = env.run(&p, None, None);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("btop"));
    fs::create_dir_all(env.0.join(".config/btop")).unwrap();
    let personal = env.0.join(".config/btop/btop.conf");
    fs::write(&personal, "personal configuration").unwrap();
    env.stub("btop", "cat \"$HOME/.config/btop/btop.conf\"");
    assert_eq!(stdout(env.run(&p, None, None)), "personal configuration");
    env.stub("btop", "exit 42");
    assert_eq!(env.run(&p, None, None).status.code(), Some(42));
    assert_eq!(
        fs::read_to_string(personal).unwrap(),
        "personal configuration"
    );
    assert!(!fs::read_dir(&env.0).unwrap().any(|f| {
        f.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("shui-monitor.")
    }));
}

#[test]
fn remote_profiles_only_launch_the_named_application() {
    let env = Sandbox::new("launch-only");
    for tool in [
        "apt-get", "dnf", "yum", "pacman", "zypper", "apk", "brew", "sudo", "curl", "wget",
    ] {
        env.stub(tool, "echo unexpected > \"$HOME/side-effect\"; exit 99");
    }
    for (name, tool, expected) in [
        ("Bash", "bash", "<-l>\n"),
        ("Tmux", "tmux", "<attach-session>\n"),
        (
            "Zellij",
            "zellij",
            "<attach>\n<--create>\n<options>\n<--show-startup-tips>\n<false>\n<--show-release-notes>\n<false>\n",
        ),
        ("Monitor", "btop", "<>\n"),
    ] {
        let mut p = profile(name);
        p.command = "sh -c %{remote}".into();
        assert!(
            !env.run(&p, None, None).status.success(),
            "{name} must fail when missing"
        );
        assert!(!env.0.join("side-effect").exists());
        env.stub(tool, "printf '<%s>\\n' \"$@\"");
        assert_eq!(stdout(env.run(&p, None, None)), expected, "{name}");
        fs::remove_file(env.0.join("bin").join(tool)).unwrap();
    }
    assert!(!env.0.join("side-effect").exists());
}

#[test]
fn container_setup_installs_and_starts_without_upgrading_the_host() {
    let env = Sandbox::new("engine-setup");
    fs::write(env.0.join("os-release"), "ID=arch\n").unwrap();
    env.stub("pacman", "printf '%s\\n' \"$@\" > \"$HOME/packages\"");
    for tool in ["usermod", "groupadd"] {
        env.stub(tool, "echo unexpected > \"$HOME/side-effect\"; exit 99");
    }
    for tool in ["systemctl", "rc-service", "rc-update"] {
        env.stub(tool, "printf '%s\\n' \"$*\" >> \"$HOME/services\"");
    }
    for (choice, binary) in [("Docker", "docker"), ("Podman", "podman")] {
        env.stub(binary, "[ \"$*\" = info ] || exit 99; echo operational");
        let before = fs::read_to_string(env.0.join("services")).unwrap_or_default();
        let mut p = profile("Install Docker / Podman");
        // Only OS discovery is replaced; execute the shipped package/install logic.
        let script = p.fields.get_mut("script").unwrap();
        *script = script.replace(". /etc/os-release", ". \"$HOME/os-release\"");
        p.command =
            "sh -c %{script} -- %{input:engine} %{input:runsc} %{input:runq} %{input:nvidia}"
                .into();
        stdout(env.run(&p, None, Some(("engine", choice))));
        let packages = fs::read_to_string(env.0.join("packages")).unwrap();
        assert!(packages.lines().any(|arg| arg == binary));
        assert!(
            !packages
                .lines()
                .any(|arg| arg.starts_with('-') && arg.contains('u'))
        );
        assert!(!env.0.join("side-effect").exists());
        let services = fs::read_to_string(env.0.join("services")).unwrap();
        if choice == "Docker" {
            assert!(
                services.contains("enable --now docker")
                    || (services.contains("docker default") && services.contains("docker start"))
            );
        } else {
            assert_eq!(services, before, "Podman is daemonless");
        }
    }
}

#[test]
fn unix_key_installation_checks_identity_and_preserves_restricted_keys() {
    let env = Sandbox::new("unix-key-install");
    for tool in ["mkdir", "cat", "awk", "chmod"] {
        env.link(tool);
    }
    env.stub(
        "ssh-keygen",
        r#"
if [ "$2" = - ]; then content=$(cat); else content=$(cat "$2"); fi
for blob in AAAA BBBB; do
  case "$content" in *"ssh-ed25519 $blob"*) printf '256 SHA256:%s fixture\n' "$blob" ;; esac
done
"#,
    );
    let mut p = profile("Configure key-based auth");
    p.command = "sh -c %{install_key}".into();
    p.inputs.clear();
    fs::create_dir_all(env.0.join(".ssh")).unwrap();
    let file = env.0.join(".ssh/authorized_keys");
    let original = "restrict ssh-ed25519 BBBB previous";
    fs::write(&file, original).unwrap();
    stdout(env.run_input(&p, None, None, b"ssh-ed25519 BBBB same-key\n"));
    assert_eq!(fs::read_to_string(&file).unwrap(), original);
    for _ in 0..2 {
        stdout(env.run_input(&p, None, None, b"ssh-ed25519 AAAA new-key\n"));
    }
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        format!("{original}\nssh-ed25519 AAAA new-key\n")
    );
    let before = fs::read(&file).unwrap();
    assert!(
        !env.run_input(&p, None, None, b"invalid-key\n")
            .status
            .success()
    );
    assert_eq!(fs::read(&file).unwrap(), before);
}

#[test]
fn new_key_generation_can_choose_an_unused_filename_without_overwriting_keys() {
    let env = Sandbox::new("new-key-filename");
    env.link("mkdir");
    fs::create_dir_all(env.0.join(".ssh")).unwrap();
    fs::write(env.0.join(".ssh/id_ed25519"), "existing private key").unwrap();
    env.stub(
        "ssh-keygen",
        r#"
while [ "$1" != -f ]; do shift; done
printf 'new private key' > "$2"
printf 'new public key' > "$2.pub"
"#,
    );
    let mut p = profile("Configure key-based auth");
    let input = p.inputs.iter_mut().find(|i| i.id == "key").unwrap();
    input.command = None;
    input.options = vec!["Generate new Ed25519 key".into()];
    p.command = "sh -c %{prepare_key} -- %{input:key} %{input:new_key}".into();
    assert!(!env.run(&p, None, None).status.success());
    let output = stdout(env.run(&p, None, Some(("new_key", "id_second"))));
    assert_eq!(
        output.trim(),
        env.0.join(".ssh/id_second.pub").to_str().unwrap()
    );
    assert_eq!(
        fs::read_to_string(env.0.join(".ssh/id_ed25519")).unwrap(),
        "existing private key"
    );
    assert!(env.0.join(".ssh/id_second").exists());
}

#[test]
fn runtime_checkboxes_select_packages_and_reject_incompatible_engines() {
    let env = Sandbox::new("runtime-options");
    fs::write(env.0.join("os-release"), "ID=arch\n").unwrap();
    env.stub(
        "pacman",
        r#"
printf '%s\n' "$@" >> "$HOME/packages"
case " $* " in *' runsc '*|*' nvidia-container-toolkit '*) exit 42 ;; esac
"#,
    );
    env.stub("docker", "echo operational");
    for service in ["systemctl", "rc-service", "rc-update"] {
        env.stub(service, "exit 0");
    }
    let mut p = profile("Install Docker / Podman");
    for input in &mut p.inputs {
        if matches!(input.id.as_str(), "runsc" | "nvidia") {
            input.default = Some("true".into());
        }
    }
    let script = p.fields.get_mut("script").unwrap();
    *script = script.replace(". /etc/os-release", ". \"$HOME/os-release\"");
    p.command =
        "sh -c %{script} -- %{input:engine} %{input:runsc} %{input:runq} %{input:nvidia}".into();
    let output = env.run(&p, None, None);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("enabled repositories"));
    let packages = fs::read_to_string(env.0.join("packages")).unwrap();
    for package in ["runsc", "nvidia-container-toolkit"] {
        assert!(packages.lines().any(|line| line == package));
    }
    fs::remove_file(env.0.join("packages")).unwrap();
    let output = env.run(&p, None, Some(("engine", "Podman")));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("require Docker"));
    assert!(!env.0.join("packages").exists());
}

#[test]
fn dotfiles_setup_installs_the_selected_shell_when_missing() {
    let env = Sandbox::new("dotfiles-shell");
    fs::create_dir_all(env.0.join("src/dotfiles")).unwrap();
    fs::write(
        env.0.join("src/dotfiles/install.sh"),
        "echo installed > \"$HOME/result\"\n",
    )
    .unwrap();
    fs::write(env.0.join("os-release"), "ID=arch\n").unwrap();
    env.stub("id", "echo 0");
    let mut p = profile("Install dotfiles");
    let script = p.fields.get_mut("script").unwrap();
    *script = script.replace(". /etc/os-release", ". \"$HOME/os-release\"");
    p.command = "sh -c %{script} -- %{input:installer} %{input:shell}".into();
    env.stub("pacman", "exit 42");
    assert_eq!(
        env.run(&p, None, Some(("shell", "bash"))).status.code(),
        Some(42)
    );
    assert!(!env.0.join("result").exists());
    env.stub(
        "pacman",
        r#"
printf '%s\n' "$*" >> "$HOME/packages"
printf '#!/bin/sh\nexec /bin/sh "$@"\n' > "$HOME/bin/bash"
/bin/chmod +x "$HOME/bin/bash"
"#,
    );
    stdout(env.run(&p, None, Some(("shell", "bash"))));
    assert_eq!(
        fs::read_to_string(env.0.join("result")).unwrap(),
        "installed\n"
    );
    let installed = fs::read_to_string(env.0.join("packages")).unwrap();
    assert!(installed.contains("bash"));
    stdout(env.run(&p, None, Some(("shell", "bash"))));
    assert_eq!(
        fs::read_to_string(env.0.join("packages")).unwrap(),
        installed
    );
}

#[test]
fn sudo_setup_installs_missing_tools_and_reuses_existing_installation() {
    let env = Sandbox::new("sudo-dependency");
    fs::write(env.0.join("os-release"), "ID=arch\n").unwrap();
    let mut p = profile("Configure passwordless sudo");
    let installer = p.fields.get_mut("install_sudo").unwrap();
    *installer = installer.replace(". /etc/os-release", ". \"$HOME/os-release\"");
    p.command = "sh -c %{install_sudo}".into();
    env.stub("pacman", "exit 42");
    assert_eq!(env.run(&p, None, None).status.code(), Some(42));
    assert!(!env.0.join("bin/sudo").exists());
    env.stub(
        "pacman",
        r#"
printf '%s\n' "$*" >> "$HOME/packages"
for tool in sudo visudo; do
  printf '#!/bin/sh\nexit 0\n' > "$HOME/bin/$tool"
  /bin/chmod +x "$HOME/bin/$tool"
done
"#,
    );
    stdout(env.run(&p, None, None));
    assert!(env.0.join("bin/sudo").exists() && env.0.join("bin/visudo").exists());
    let installed = fs::read_to_string(env.0.join("packages")).unwrap();
    stdout(env.run(&p, None, None));
    assert_eq!(
        fs::read_to_string(env.0.join("packages")).unwrap(),
        installed
    );
    fs::remove_file(env.0.join("bin/visudo")).unwrap();
    stdout(env.run(&p, None, None));
    assert!(env.0.join("bin/visudo").exists());
}

#[test]
fn terminal_setup_installs_missing_tic_before_sending_definition() {
    let env = Sandbox::new("terminfo-dependency");
    for tool in ["cat", "mktemp", "mkdir", "rm"] {
        env.link(tool);
    }
    fs::write(env.0.join("os-release"), "ID=arch\n").unwrap();
    env.stub("id", "echo 0");
    env.stub(
        "ssh",
        "[ \"$1\" = -p ] && shift 2; [ \"$1\" = -tt ] && shift; shift 2; exec /bin/sh -c \"$1\"",
    );
    env.stub("infocmp", "echo 'test-terminal|test terminal, cols#80,'");
    let mut p = profile("Copy terminal definition");
    let installer = p.fields.get_mut("ensure_tic").unwrap();
    *installer = installer.replace(". /etc/os-release", ". \"$HOME/os-release\"");
    env.stub("pacman", "exit 42");
    assert_eq!(env.run(&p, None, None).status.code(), Some(42));
    assert!(!env.0.join(".terminfo").exists());
    env.stub(
        "pacman",
        r#"
printf '%s\n' "$*" >> "$HOME/packages"
printf '#!/bin/sh\ncat "$4"\n' > "$HOME/bin/tic"
/bin/chmod +x "$HOME/bin/tic"
"#,
    );
    assert_eq!(
        stdout(env.run(&p, Some("2222"), None)),
        "test-terminal|test terminal, cols#80,\n"
    );
    let installed = fs::read_to_string(env.0.join("packages")).unwrap();
    assert!(installed.contains("ncurses"));
    stdout(env.run(&p, None, None));
    assert_eq!(
        fs::read_to_string(env.0.join("packages")).unwrap(),
        installed
    );
}

#[test]
fn terminal_definition_is_exported_before_ssh_and_temp_files_are_removed() {
    let env = Sandbox::new("terminfo");
    for utility in ["cat", "mktemp", "mkdir", "rm"] {
        env.link(utility);
    }
    // Emulate ssh's remote shell, forwarding stdin without executing any real ssh.
    env.stub(
        "ssh",
        "[ \"$1\" = -p ] && shift 2; [ \"$1\" = -tt ] && shift; shift 2; exec /bin/sh -c \"$1\"",
    );
    env.stub("infocmp", "echo 'test-terminal|test terminal, cols#80,'");
    env.stub("tic", "[ \"$1\" = -x ] && [ \"$2\" = -o ] && [ \"$3\" = \"$HOME/.terminfo\" ] || exit 9; cat \"$4\"");
    let p = profile("Copy terminal definition");
    assert_eq!(
        stdout(env.run(&p, Some("2222"), None)),
        "test-terminal|test terminal, cols#80,\n"
    );
    assert!(env.0.join(".terminfo").is_dir());
    env.stub("tic", "exit 42");
    assert_eq!(env.run(&p, None, None).status.code(), Some(42));
    assert!(!fs::read_dir(&env.0).unwrap().any(|f| {
        f.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("shui-terminfo.")
    }));
    env.stub("ssh", "touch \"$HOME/contacted\"");
    env.link("touch");
    env.stub("infocmp", "echo partial; exit 1");
    assert!(!env.run(&p, None, None).status.success());
    assert!(!env.0.join("contacted").exists());
}

#[test]
fn bundled_recipe_payloads_parse_in_their_documented_shell() {
    let config = Config::parse(&crate::config_defaults::defaults(false, false).unwrap()).unwrap();
    for recipe in config.configuration_scripts {
        for (field, source) in &recipe.fields {
            if !matches!(
                field.as_str(),
                "script"
                    | "bootstrap"
                    | "maintenance"
                    | "firewall"
                    | "install_sudo"
                    | "prepare"
                    | "prepare_key"
                    | "setup_script"
                    | "set_expiry"
                    | "ensure_tic"
                    | "elevate"
                    | "export_script"
            ) {
                continue;
            }
            let shell = if field == "maintenance" { "bash" } else { "sh" };
            let mut child = Command::new(shell)
                .arg("-n")
                .stdin(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(source.as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{}/{field}: {}",
                recipe.name,
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[test]
fn shell_firewall_validation_rejects_bad_inputs_before_changing_rules() {
    let env = Sandbox::new("firewall-validation");
    for tool in ["tr", "awk", "grep"] {
        env.link(tool);
    }
    // The real iproute2 parser is exercised in the Linux service integration.
    env.stub("ip", "exit 1");
    for tool in ["ufw", "firewall-cmd", "systemctl"] {
        env.stub(tool, "echo unexpected > \"$HOME/changed\"; exit 99");
    }
    let mut p = profile("Configure firewall");
    p.command = "sh -c %{firewall} -- ufw %{input:ssh_ips} %{input:ports} 2222".into();
    // Bypass the client format checks to exercise the remote checks as well.
    for input in &mut p.inputs {
        input.pattern = None;
    }
    for (id, value, message) in [
        ("ssh_ips", "not-an-ip", "Invalid SSH source"),
        ("ssh_ips", "999.2.3.4", "Invalid SSH source"),
        ("ssh_ips", "192.0.2.1/33", "Invalid SSH source"),
        ("ssh_ips", "2001:::1", "Invalid SSH source"),
        ("ports", "70000/tcp", "Port range"),
        ("ports", "9000-8000/tcp", "Port range"),
        ("ports", "2200-2300/tcp", "Configure SSH access"),
        ("ports", "2222/udp", "Configure SSH access"),
        ("ports", "80/tcp;echo", "Ports require"),
    ] {
        let out = env.run(&p, None, Some((id, value)));
        assert!(!out.status.success(), "{value}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(message),
            "{value}: {:?}",
            out.stderr
        );
        assert!(!env.0.join("changed").exists());
    }
}

#[test]
fn shell_log_validation_rejects_invalid_limits_before_maintenance() {
    let env = Sandbox::new("log-validation");
    env.link("bash");
    env.stub("jq", "echo unexpected > \"$HOME/changed\"; exit 99");
    let mut p = profile("Configure log limits and cleanup");
    p.command = "bash -c %{maintenance} -- unused-bootstrap unused-source %{input:disk} %{input:runtime} %{input:days} %{input:file_size} %{input:backups} %{input:cleanup} %{input:all_archives} %{input:package_caches} %{input:user_caches} %{input:all_users} %{input:guests} %{input:depth}".into();
    for input in &mut p.inputs {
        input.pattern = None;
    }
    for (id, value, message) in [
        ("disk", "0", "positive bytes"),
        ("disk", "100000000000000000000T", "too large"),
        ("file_size", "10MB", "positive bytes"),
        ("days", "36501", "Days must"),
        ("backups", "101", "Backups must"),
        ("depth", "6", "Guest depth"),
    ] {
        let out = env.run(&p, None, Some((id, value)));
        assert!(!out.status.success(), "{value}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(message),
            "{value}: {:?}",
            out.stderr
        );
        assert!(!env.0.join("changed").exists());
    }
}

#[test]
fn shipped_profiles_have_no_preparation_side_effects() {
    for windows in [false, true] {
        let config =
            Config::parse(&crate::config_defaults::defaults(windows, false).unwrap()).unwrap();
        for p in config.profiles {
            assert!(
                p.prepare(&Target {
                    host: "unused.invalid".into(),
                    port: None
                })
                .unwrap()
                .is_none(),
                "{}",
                p.name
            );
        }
    }
}

#[test]
fn key_expiry_updates_all_matching_entries_and_preserves_restrictions() {
    let env = Sandbox::new("key-expiry");
    for tool in ["awk", "mktemp", "cp", "cmp", "chmod", "mv", "rm"] {
        env.link(tool);
    }
    let date = Command::new("sh")
        .args(["-c", "command -v date"])
        .output()
        .unwrap();
    let date = String::from_utf8(date.stdout).unwrap();
    env.stub(
        "date",
        &format!(
            "if [ \"$1\" = +%s ]; then echo 1704067200; else exec {} \"$@\"; fi",
            date.trim()
        ),
    );
    fs::create_dir_all(env.0.join(".ssh")).unwrap();
    let authorized = env.0.join(".ssh/authorized_keys");
    let prefix = "# ssh-ed25519 AAAATEST commented out\n\nssh-ed25519 OTHER unrelated\n";
    let restrictions = r#"command="echo \"quoted, text\" expiry-time=keep",no-agent-forwarding"#;
    let original = format!(
        "{prefix}{restrictions},expiry-time=\"20200101Z\" ssh-ed25519 AAAATEST comment with spaces\nssh-ed25519 AAAATEST duplicate\n"
    );
    let mut p = profile("Configure key-based auth");
    p.inputs.retain(|input| input.id == "expiry");
    p.command = "sh -c %{set_expiry} -- %{input:expiry}".into();
    for (choice, expiry) in [
        ("8 hours", "20240101080000Z"),
        ("day", "20240102000000Z"),
        ("week", "20240108000000Z"),
        ("month", "20240131000000Z"),
        ("year", "20241231000000Z"),
        ("never", ""),
    ] {
        fs::write(&authorized, &original).unwrap();
        stdout(env.run_input(
            &p,
            None,
            Some(("expiry", choice)),
            b"ssh-ed25519 AAAATEST local comment\n",
        ));
        let option = if expiry.is_empty() {
            String::new()
        } else {
            format!("expiry-time=\"{expiry}\"")
        };
        let comma = if option.is_empty() { "" } else { "," };
        let space = if option.is_empty() { "" } else { " " };
        assert_eq!(
            fs::read_to_string(&authorized).unwrap(),
            format!(
                "{prefix}{restrictions}{comma}{option} ssh-ed25519 AAAATEST comment with spaces\n{option}{space}ssh-ed25519 AAAATEST duplicate\n"
            )
        );
        assert_eq!(
            fs::metadata(&authorized).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    // Missing keys and malformed choices must leave the file unchanged.
    fs::write(&authorized, &original).unwrap();
    assert!(
        !env.run_input(&p, None, Some(("expiry", "week")), b"ssh-ed25519 MISSING\n")
            .status
            .success()
    );
    assert_eq!(fs::read_to_string(&authorized).unwrap(), original);
    p.command = "sh -c %{set_expiry} -- invalid".into();
    assert!(
        !env.run_input(&p, None, None, b"ssh-ed25519 AAAATEST\n")
            .status
            .success()
    );
    assert_eq!(fs::read_to_string(&authorized).unwrap(), original);
    assert!(!fs::read_dir(env.0.join(".ssh")).unwrap().any(|f| {
        f.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".shui-expiry.")
    }));
    let dotfiles = profile("Install dotfiles");
    assert!(
        dotfiles
            .inputs
            .iter()
            .all(|i| i.id != "key" && i.id != "expiry")
    );
}

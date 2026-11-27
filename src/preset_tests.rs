//! Execute shipped shell recipes with isolated PATHs; never contact a server.
use crate::{
    Target,
    launch::{Config, Profile},
};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    process::{Command, Output},
};

struct Sandbox(PathBuf);
impl Sandbox {
    fn new(name: &str) -> Self {
        // Match the physical paths reported by find -execdir when TMPDIR is a symlink.
        let temp = std::env::temp_dir().canonicalize().unwrap();
        let root = temp.join(format!("shui-presets-{name}-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("bin")).unwrap();
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
        assert!(found.status.success(), "missing recipe test tool: {name}");
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
        let mut profile = profile.clone();
        if self.0.join("os-release").exists() {
            for source in profile.fields.values_mut() {
                *source = source.replace(
                    "/etc/os-release",
                    &format!("{}/os-release", self.0.display()),
                );
            }
        }
        let mut values = profile.input_defaults();
        if let Some((id, value)) = input {
            values.insert(id.into(), value.into());
        }
        let (program, args) = profile
            .command_with_inputs(
                &Target {
                    host: "user@host".into(),
                    port: port.map(str::to_owned),
                },
                &values,
            )
            .unwrap();
        let input_path = self.0.join("stdin");
        fs::write(&input_path, stdin).unwrap();
        Command::new(program)
            .current_dir(&self.0)
            .args(args)
            .env("PATH", self.0.join("bin"))
            .env("HOME", &self.0)
            .env("TMPDIR", &self.0)
            .env("TERM", "test-terminal")
            .stdin(fs::File::open(input_path).unwrap())
            .output()
            .unwrap()
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
fn key_installation_does_not_mistake_comments_for_authorized_keys() {
    let env = Sandbox::new("key-comment-regression");
    for tool in ["mkdir", "touch", "chmod", "tr", "awk", "ssh-keygen"] {
        env.link(tool);
    }
    assert!(
        Command::new(env.0.join("bin/ssh-keygen"))
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(env.0.join("key"))
            .status()
            .unwrap()
            .success()
    );
    let key = fs::read_to_string(env.0.join("key.pub")).unwrap();
    fs::create_dir(env.0.join(".ssh")).unwrap();
    let file = env.0.join(".ssh/authorized_keys");
    for windows in [true, false] {
        let config =
            Config::parse(&crate::config_defaults::defaults(windows, false).unwrap()).unwrap();
        let mut p = config.configuration_scripts[0].clone();
        p.shell = crate::command::Shell::Sh;
        p.command = "sh -c %{install_key}".into();
        p.inputs.clear();
        let comment = format!("# client key: {key}");
        fs::write(&file, &comment).unwrap();
        for _ in 0..2 {
            stdout(env.run_input(&p, None, None, key.as_bytes()));
        }
        let installed = fs::read_to_string(&file).unwrap();
        assert_eq!(
            installed
                .lines()
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .count(),
            1,
            "windows={windows}: {installed}"
        );
        let before = installed;
        assert!(
            !env.run_input(&p, None, None, b"not-a-public-key\n")
                .status
                .success()
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), before);
    }
}

#[test]
fn windows_key_installation_preserves_existing_keys_and_is_idempotent() {
    let env = Sandbox::new("windows-key-install");
    for tool in ["mkdir", "touch", "chmod", "tr", "awk"] {
        env.link(tool);
    }
    env.stub("ssh-keygen", "if [ \"$2\" = - ]; then content=$(cat); else content=$(cat \"$2\"); fi; for blob in AAAA BBBB; do case \"$content\" in *\"ssh-ed25519 $blob\"*) printf '256 SHA256:%s fixture\\n' \"$blob\" ;; esac; done");
    env.link("cat");
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
        let profile = profile(name);
        let (program, args) = profile
            .shell
            .command(profile.available_command.unwrap())
            .unwrap();
        let available = |os| {
            Command::new(&program)
                .args(&args)
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
    env.stub("dockerd", "exit 0");
    fs::write(env.0.join("os-release"), "ID=arch\n").unwrap();
    env.stub("pacman", "printf '%s\\n' \"$@\" >> \"$HOME/packages\"; for package do case \"$package\" in docker|podman) printf '#!/bin/sh\\necho operational\\n' > \"$HOME/bin/$package\"; /bin/chmod +x \"$HOME/bin/$package\" ;; esac; done");
    for tool in ["usermod", "groupadd"] {
        env.stub(tool, "echo unexpected > \"$HOME/side-effect\"; exit 99");
    }
    for tool in ["systemctl", "rc-service", "rc-update"] {
        env.stub(tool, "printf '%s\\n' \"$*\" >> \"$HOME/services\"");
    }
    for (choice, binary) in [("Docker", "docker"), ("Podman", "podman")] {
        let before = fs::read_to_string(env.0.join("services")).unwrap_or_default();
        let mut p = profile("Install Docker / Podman");
        // Only OS discovery is replaced; execute the shipped package/install logic.
        let script = p.fields.get_mut("script").unwrap();
        *script = script.replace(". /etc/os-release", ". \"$HOME/os-release\"");
        p.command =
            "sh -c %{script} -- %{input:engine} %{input:runsc} %{input:runq} %{input:nvidia} %{packages}"
                .into();
        stdout(env.run(&p, None, Some(("engine", choice))));
        let packages = fs::read_to_string(env.0.join("packages")).unwrap();
        assert!(packages.lines().any(|arg| arg == binary));
        stdout(env.run(&p, None, Some(("engine", choice))));
        assert_eq!(
            fs::read_to_string(env.0.join("packages")).unwrap(),
            packages,
            "an installed engine must not be reinstalled"
        );
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
    for tool in ["mkdir", "cat", "awk", "chmod", "tr"] {
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
fn debian_docker_setup_installs_the_separate_client_when_needed() {
    let env = Sandbox::new("docker-client");
    fs::write(
        env.0.join("os-release"),
        "ID=debian\nVERSION_CODENAME=trixie\n",
    )
    .unwrap();
    env.stub("apt-cache", "[ \"$*\" = 'show docker-cli' ]");
    env.stub(
        "apt-get",
        r#"
case "$1" in
 update) exit 0 ;;
 indextargets) echo 'Debian|trixie' ;;
 install)
  for package do
   case "$package" in
    docker.io) binary=dockerd ;;
    docker-cli) binary=docker ;;
    *) continue ;;
   esac
   echo "$package" >> "$HOME/packages"
   printf '#!/bin/sh\necho operational\n' > "$HOME/bin/$binary"
   /bin/chmod +x "$HOME/bin/$binary"
  done ;;
 *) exit 99 ;;
esac
"#,
    );
    for service in ["systemctl", "rc-service", "rc-update"] {
        env.stub(service, "exit 0");
    }
    let mut p = profile("Install Docker / Podman");
    p.command = "sh -c %{script} -- %{input:engine} %{input:runsc} %{input:runq} %{input:nvidia} %{packages}".into();
    stdout(env.run(&p, None, None));
    assert_eq!(
        fs::read_to_string(env.0.join("packages")).unwrap(),
        "docker.io\ndocker-cli\n"
    );
    stdout(env.run(&p, None, None));
    assert_eq!(
        fs::read_to_string(env.0.join("packages")).unwrap(),
        "docker.io\ndocker-cli\n"
    );
}

#[test]
fn podman_installs_only_its_missing_native_default_runtime() {
    let env = Sandbox::new("podman-runtime");
    fs::write(env.0.join("os-release"), "ID=arch\n").unwrap();
    env.stub("podman", "if [ ! -f \"$HOME/bin/crun\" ]; then echo 'Error: default OCI runtime \"crun\" not found: invalid argument' >&2; exit 125; fi; echo operational");
    env.stub("pacman", "printf '%s\\n' \"$*\" >> \"$HOME/packages\"; printf '#!/bin/sh\\nexit 0\\n' > \"$HOME/bin/crun\"; /bin/chmod +x \"$HOME/bin/crun\"");
    let mut p = profile("Install Docker / Podman");
    p.command = "sh -c %{script} -- %{input:engine} %{input:runsc} %{input:runq} %{input:nvidia} %{packages}".into();
    stdout(env.run(&p, None, Some(("engine", "Podman"))));
    let installed = fs::read_to_string(env.0.join("packages")).unwrap();
    assert!(installed.contains("crun"));
    assert!(!installed.contains("podman") && !installed.contains("firewalld"));
    stdout(env.run(&p, None, Some(("engine", "Podman"))));
    assert_eq!(
        fs::read_to_string(env.0.join("packages")).unwrap(),
        installed
    );
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
        "sh -c %{script} -- %{input:engine} %{input:runsc} %{input:runq} %{input:nvidia} %{packages}".into();
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
fn docker_runtime_registration_waits_for_reload_without_restarting() {
    let env = Sandbox::new("docker-runtime-reload");
    for tool in ["jq", "mkdir", "mktemp", "cat", "chmod", "mv", "rm"] {
        env.link(tool);
    }
    fs::write(env.0.join("os-release"), "ID=arch\n").unwrap();
    fs::create_dir(env.0.join("systemd")).unwrap();
    env.stub("runsc", "exit 0");
    env.stub("dockerd", "exit 0");
    env.stub("sleep", "echo waited >> \"$HOME/waits\"");
    env.stub("systemctl", "printf '%s\\n' \"$*\" >> \"$HOME/services\"");
    env.stub(
        "docker",
        r#"
[ "$#" -gt 1 ] || exit 0
if [ -f "$HOME/queried" ]; then echo '{"runsc":{}}'; else echo '{}'; fi
: > "$HOME/queried"
"#,
    );
    let mut p = profile("Install Docker / Podman");
    for source in p.fields.values_mut() {
        *source = source
            .replace(
                "/run/systemd/system",
                &format!("{}/systemd", env.0.display()),
            )
            .replace("/etc/docker", &format!("{}/docker", env.0.display()));
    }
    p.command = "sh -c %{script} -- %{input:engine} %{input:runsc} %{input:runq} %{input:nvidia} %{packages}".into();
    stdout(env.run(&p, None, Some(("runsc", "true"))));
    assert_eq!(fs::read_to_string(env.0.join("waits")).unwrap(), "waited\n");
    assert!(
        !fs::read_to_string(env.0.join("services"))
            .unwrap()
            .contains("restart")
    );
    env.link("cp");
    env.link("stat");
    env.stub("docker", "echo '{}'");
    fs::remove_file(env.0.join("waits")).unwrap();
    let out = env.run(&p, None, Some(("runsc", "true")));
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("did not register"));
    assert_eq!(
        fs::read_to_string(env.0.join("waits"))
            .unwrap()
            .lines()
            .count(),
        4
    );
}

#[test]
fn dotfiles_archive_preserves_links_without_copying_their_targets() {
    let env = Sandbox::new("dotfiles-links");
    for tool in ["tar", "mktemp", "rm", "mkdir"] {
        env.link(tool);
    }
    fs::create_dir(env.0.join("dotfiles")).unwrap();
    fs::create_dir(env.0.join("outside")).unwrap();
    fs::create_dir(env.0.join("remote")).unwrap();
    fs::write(env.0.join("outside/secret"), "not part of dotfiles\n").unwrap();
    symlink(env.0.join("outside"), env.0.join("dotfiles/outside-link")).unwrap();
    fs::write(
        env.0.join("dotfiles/install.sh"),
        "test -L outside-link; echo installed > \"$HOME/installed\"\n",
    )
    .unwrap();
    env.stub("ssh", "[ \"$1\" = -p ] && shift 2; [ \"$1\" = -tt ] && shift; shift 2; HOME=\"$HOME/remote\" /bin/sh -c \"$1\"");
    env.stub("scp", "echo unexpected > \"$HOME/scp-used\"; exit 99");
    let p = profile("Install dotfiles");
    stdout(env.run(&p, None, Some(("source", "dotfiles"))));
    assert!(
        fs::symlink_metadata(env.0.join("remote/src/dotfiles/outside-link"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(env.0.join("remote/installed").exists());
    assert!(!env.0.join("scp-used").exists());
    assert!(!fs::read_dir(&env.0).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("shui-dotfiles.")
    }));
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
    p.command = "sh -c %{script} -- %{input:installer} %{input:shell} %{packages}".into();
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
fn passwordless_sudo_grants_only_the_selected_scope() {
    let env = Sandbox::new("sudo-scope");
    for tool in ["mkdir", "cp", "mv", "chmod", "mktemp", "rm"] {
        env.link(tool);
    }
    fs::write(env.0.join("os-release"), "ID=debian\n").unwrap();
    fs::create_dir(env.0.join("sudoers.d")).unwrap();
    env.stub("id", "[ \"$*\" = -un ] && echo alice; exit 0");
    env.stub("getent", "exit 0");
    env.stub("sudo", "exit 0");
    env.stub("visudo", "exit 0");
    env.stub(
        "groupadd",
        "echo unexpected > \"$HOME/group-change\"; exit 99",
    );
    let mut p = profile("Configure passwordless sudo");
    let script = p.fields.get_mut("script").unwrap();
    *script = script.replace("/etc/sudoers.d", &format!("{}/sudoers.d", env.0.display()));
    p.command =
        "sh -c %{script} -- %{install_sudo} %{packages} %{input:scope} %{input:account}".into();
    stdout(env.run(&p, None, Some(("account", "alice"))));
    assert_eq!(
        fs::read_to_string(env.0.join("sudoers.d/shui-nopasswd-user-alice")).unwrap(),
        "alice ALL=(ALL:ALL) NOPASSWD: ALL\n"
    );
    assert!(!env.0.join("sudoers.d/shui-nopasswd-group-sudo").exists());
    assert!(!env.0.join("group-change").exists());
    fs::write(env.0.join("os-release"), "ID=customlinux\n").unwrap();
    stdout(env.run(&p, None, Some(("account", "alice"))));
    fs::write(env.0.join("os-release"), "ID=debian\n").unwrap();
    stdout(env.run(&p, None, Some(("scope", "Administrative group"))));
    assert_eq!(
        fs::read_to_string(env.0.join("sudoers.d/shui-nopasswd-group-sudo")).unwrap(),
        "%sudo ALL=(ALL:ALL) NOPASSWD: ALL\n"
    );
    env.stub("id", "exit 1");
    assert!(
        !env.run(&p, None, Some(("account", "missing")))
            .status
            .success()
    );
}

#[test]
fn sudo_setup_installs_missing_tools_and_reuses_existing_installation() {
    let env = Sandbox::new("sudo-dependency");
    fs::write(env.0.join("os-release"), "ID=arch\n").unwrap();
    let mut p = profile("Configure passwordless sudo");
    let installer = p.fields.get_mut("install_sudo").unwrap();
    *installer = installer.replace(". /etc/os-release", ". \"$HOME/os-release\"");
    p.command = "sh -c %{install_sudo} -- %{packages}".into();
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
                    | "traverse"
                    | "packages"
            ) {
                continue;
            }
            let shell = if field == "maintenance" { "bash" } else { "sh" };
            let output = Command::new(shell)
                .args(["-n", "-c", source])
                .output()
                .unwrap();
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
fn dependency_installation_rejects_foreign_apt_releases_and_limits_changes() {
    let env = Sandbox::new("dependency-release-guard");
    env.link("cat");
    fs::write(
        env.0.join("os-release"),
        "ID=debian\nVERSION_CODENAME=bookworm\n",
    )
    .unwrap();
    env.stub("apt-get", "case \"$1\" in update) exit 0 ;; indextargets) cat \"$HOME/indexes\" ;; install) printf '%s\\n' \"$*\" >> \"$HOME/installed\" ;; *) exit 99 ;; esac");
    let mut p = profile("Install dotfiles");
    p.command = "sh -c %{packages} -- bash".into();
    for indexes in [
        "Debian|trixie\n",
        "Debian|bookworm\nDebian|trixie-security\n",
        "",
    ] {
        fs::write(env.0.join("indexes"), indexes).unwrap();
        assert!(!env.run(&p, None, None).status.success());
        assert!(!env.0.join("installed").exists());
    }
    fs::write(
        env.0.join("indexes"),
        "Debian|bookworm\nDebian|bookworm-security\nDebian Backports|bookworm-backports\n",
    )
    .unwrap();
    stdout(env.run(&p, None, None));
    let action = fs::read_to_string(env.0.join("installed")).unwrap();
    for flag in ["--no-upgrade", "--no-remove", "--no-install-recommends"] {
        assert!(action.contains(flag));
    }
    assert!(!action.contains("dist-upgrade") && !action.contains("autoremove"));
}

#[test]
fn logging_bootstrap_requests_only_missing_tools() {
    let env = Sandbox::new("logging-dependencies");
    for tool in ["bash", "find"] {
        env.link(tool);
    }
    env.stub("timeout", "exit 0");
    let mut p = profile("Configure log limits and cleanup");
    p.fields.insert(
        "packages".into(),
        "[ \"$#\" -gt 0 ] || exit 0; printf '%s\\n' \"$@\" >> \"$HOME/installed\"".into(),
    );
    p.command = "sh -c %{bootstrap} -- %{bootstrap} %{maintenance} %{packages} %{input:disk} %{input:runtime} %{input:days} %{input:file_size} %{input:backups} %{input:cleanup} %{input:all_archives} %{input:package_caches} %{input:user_caches} %{input:all_users} %{input:guests} %{input:depth}".into();
    stdout(env.run(&p, None, None));
    assert_eq!(
        fs::read_to_string(env.0.join("installed")).unwrap(),
        "logrotate\n"
    );
    env.stub("logrotate", "exit 0");
    stdout(env.run(&p, None, None));
    assert_eq!(
        fs::read_to_string(env.0.join("installed")).unwrap(),
        "logrotate\n"
    );
    fs::remove_file(env.0.join("bin/find")).unwrap();
    for input in &mut p.inputs {
        if input.id == "cleanup" {
            input.default = Some("false".into());
        }
    }
    stdout(env.run(&p, None, None));
    assert_eq!(
        fs::read_to_string(env.0.join("installed")).unwrap(),
        "logrotate\n"
    );
    for option in ["cleanup", "user_caches"] {
        fs::remove_file(env.0.join("installed")).unwrap();
        stdout(env.run(&p, None, Some((option, "true"))));
        assert_eq!(
            fs::read_to_string(env.0.join("installed")).unwrap(),
            "findutils\n"
        );
    }
}

#[test]
fn firewall_validates_before_installation_and_restores_ufw_after_failure() {
    let env = Sandbox::new("firewall-rollback");
    for tool in [
        "cp", "mkdir", "mktemp", "tr", "awk", "grep", "sed", "rm", "sort",
    ] {
        env.link(tool);
    }
    fs::write(env.0.join("os-release"), "ID=debian\n").unwrap();
    fs::create_dir(env.0.join("systemd")).unwrap();
    fs::create_dir(env.0.join("ufw")).unwrap();
    fs::write(env.0.join("ufw/user.rules"), "original rules\n").unwrap();
    fs::write(env.0.join("default-ufw"), "IPV6=no\n").unwrap();
    env.stub("ip", "case \"$*\" in *999*) exit 1 ;; *) exit 0 ;; esac");
    env.stub(
        "systemctl",
        "case \"$*\" in 'is-active --quiet firewalld') exit 1 ;; esac; printf '%s\\n' \"$*\" >> \"$HOME/service-actions\"",
    );
    env.stub(
        "ufw",
        r#"
printf '%s\n' "$*" >> "$HOME/actions"
case "$*" in
 status) echo 'Status: active' ;;
 'status numbered') printf '[ 1] 22/tcp  ALLOW IN  Anywhere\n[ 2] 53/udp  ALLOW OUT  Anywhere # ALLOW IN\n[ 3] 80/tcp  ALLOW FWD  Anywhere\n[ 4] 443/tcp  ALLOW IN  Anywhere\n' ;;
 '--force delete '*) echo changed > "$HOME/ufw/user.rules" ;;
 'prepend allow'*) echo failed > "$HOME/failed"; exit 42 ;;
 '--force enable') [ ! -f "$HOME/fail-restore" ] || [ ! -f "$HOME/failed" ] || exit 43; echo active > "$HOME/state" ;;
esac
"#,
    );
    let mut p = profile("Configure firewall");
    for source in p.fields.values_mut() {
        *source = source
            .replace(
                "/run/systemd/system",
                &format!("{}/systemd", env.0.display()),
            )
            .replace(
                "/etc/default/ufw",
                &format!("{}/default-ufw", env.0.display()),
            )
            .replace("/etc/ufw", &format!("{}/ufw", env.0.display()));
    }
    p.fields.insert(
        "packages".into(),
        "echo unexpected > \"$HOME/installed\"; exit 99".into(),
    );
    p.command =
        "sh -c %{script} -- %{firewall} %{input:ssh_ips} %{input:ports} 22 %{packages}".into();
    assert!(
        !env.run(&p, None, Some(("ssh_ips", "999.999.999.999")))
            .status
            .success()
    );
    assert!(!env.0.join("installed").exists() && !env.0.join("actions").exists());
    let output = env.run(&p, None, None);
    assert_eq!(output.status.code(), Some(42), "{:?}", output.stderr);
    assert_eq!(
        fs::read_to_string(env.0.join("ufw/user.rules")).unwrap(),
        "original rules\n"
    );
    assert_eq!(
        fs::read_to_string(env.0.join("default-ufw")).unwrap(),
        "IPV6=no\n"
    );
    assert_eq!(fs::read_to_string(env.0.join("state")).unwrap(), "active\n");
    assert!(!env.0.join("installed").exists());
    let actions = fs::read_to_string(env.0.join("actions")).unwrap();
    assert!(actions.contains("--force delete 4\n--force delete 1\n"));
    assert!(!actions.contains("--force reset") && !actions.contains("default allow outgoing"));
    fs::remove_file(env.0.join("failed")).unwrap();
    fs::write(env.0.join("fail-restore"), "").unwrap();
    let output = env.run(&p, None, None);
    assert_eq!(output.status.code(), Some(42));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Backups retained"));
    assert!(
        fs::read_dir(&env.0).unwrap().any(|entry| entry
            .unwrap()
            .path()
            .join("ufw/user.rules")
            .exists())
    );
    env.stub("ufw", "exit 1");
    let output = env.run(&p, None, None);
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(env.0.join("ufw/user.rules")).unwrap(),
        "original rules\n"
    );
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
fn automatic_security_updates_offer_native_reboot_without_rebooting_during_setup() {
    let env = Sandbox::new("automatic-security-reboot");
    for tool in ["cat", "mkdir"] {
        env.link(tool);
    }
    for directory in ["apt", "systemd"] {
        fs::create_dir(env.0.join(directory)).unwrap();
    }
    env.stub(
        "unattended-upgrade",
        "echo unexpected-update >> \"$HOME/actions\"; exit 1",
    );
    env.stub("systemctl", "printf '%s\\n' \"$*\" >> \"$HOME/actions\"");
    env.stub(
        "apt-get",
        "echo unexpected-package-operation >> \"$HOME/actions\"; exit 99",
    );
    let mut p = profile("Configure automatic security updates");
    let script = p.fields.get_mut("script").unwrap();
    for (from, to) in [
        ("/etc/os-release", "os-release"),
        ("/etc/apt/apt.conf.d", "apt"),
        ("/etc/systemd/system", "systemd"),
        ("/run/systemd/system", "systemd"),
    ] {
        *script = script.replace(from, &format!("{}/{to}", env.0.display()));
    }
    p.command = "sh -c %{script} -- %{input:reboot}".into();
    assert_eq!(p.input_defaults()["reboot"], "false");
    for distro in ["debian", "ubuntu"] {
        fs::write(env.0.join("os-release"), format!("ID={distro}\n")).unwrap();
        for reboot in ["true", "false"] {
            let output = stdout(env.run(&p, None, Some(("reboot", reboot))));
            let policy = fs::read_to_string(env.0.join("apt/52shui-security-updates")).unwrap();
            assert!(policy.contains(&format!("Automatic-Reboot \"{reboot}\";")));
            assert!(policy.contains("codename=${distro_codename}-security"));
            assert!(output.contains(&format!("automatic reboot: {reboot}")));
            for setting in [
                "Remove-Unused-Kernel-Packages",
                "Remove-New-Unused-Dependencies",
                "Remove-Unused-Dependencies",
            ] {
                assert!(policy.contains(&format!("{setting} \"false\";")));
            }
        }
    }
    let actions = fs::read_to_string(env.0.join("actions")).unwrap();
    assert!(!actions.contains("unexpected-update") && !actions.contains("reboot"));
    assert!(!actions.contains("unexpected-package-operation"));
    assert!(actions.contains("enable --now apt-daily.timer apt-daily-upgrade.timer"));
    assert!(!env.0.join("systemd/shui-security-updates.service").exists());
    let policy = fs::read(env.0.join("apt/52shui-security-updates")).unwrap();
    env.stub(
        "systemctl",
        "case \"$*\" in *dnf*|*cron*) exit 1 ;; esac; printf '%s\\n' \"$*\" >> \"$HOME/actions\"",
    );
    let actions = fs::read_to_string(env.0.join("actions")).unwrap();
    for distro in ["fedora", "opensuse-leap", "alpine", "arch", "unknown"] {
        fs::write(env.0.join("os-release"), format!("ID={distro}\n")).unwrap();
        let output = env.run(&p, None, Some(("reboot", "true")));
        assert!(output.status.success(), "{distro}: {:?}", output.stderr);
        assert!(String::from_utf8_lossy(&output.stderr).contains("Update this system manually"));
        assert_eq!(fs::read_to_string(env.0.join("actions")).unwrap(), actions);
        assert_eq!(
            fs::read(env.0.join("apt/52shui-security-updates")).unwrap(),
            policy
        );
    }
    fs::write(
        env.0.join("os-release"),
        "ID=debian\nVERSION_CODENAME=bookworm\n",
    )
    .unwrap();
    fs::remove_file(env.0.join("bin/unattended-upgrade")).unwrap();
    let output = env.run(&p, None, None);
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("no packages or configuration were changed")
    );
    assert_eq!(fs::read_to_string(env.0.join("actions")).unwrap(), actions);
    assert_eq!(
        fs::read(env.0.join("apt/52shui-security-updates")).unwrap(),
        policy
    );
}

#[test]
fn automatic_security_updates_use_native_dnf_policy_and_preserve_other_settings() {
    let env = Sandbox::new("native-dnf-security");
    for tool in ["awk", "grep", "mktemp", "cp", "mv", "rm"] {
        env.link(tool);
    }
    for directory in ["dnf", "systemd"] {
        fs::create_dir(env.0.join(directory)).unwrap();
    }
    env.stub(
        "dnf",
        "echo unexpected-package-operation >> \"$HOME/actions\"; exit 99",
    );
    env.stub(
        "dnf5",
        "echo unexpected-package-operation >> \"$HOME/actions\"; exit 99",
    );
    let mut p = profile("Configure automatic security updates");
    let script = p.fields.get_mut("script").unwrap();
    for (from, to) in [
        ("/etc/os-release", "os-release"),
        ("/etc/dnf", "dnf"),
        ("/etc/systemd/system", "systemd"),
        ("/run/systemd/system", "systemd"),
    ] {
        *script = script.replace(from, &format!("{}/{to}", env.0.display()));
    }
    p.command = "sh -c %{script} -- %{input:reboot}".into();
    for timer in ["dnf5-automatic.timer", "dnf-automatic-install.timer"] {
        env.stub("systemctl", &format!("case \"$*\" in 'cat {timer}') exit 0 ;; cat*) exit 1 ;; esac; printf '%s\\n' \"$*\" >> \"$HOME/actions\""));
        for distro in ["fedora", "rocky", "ol"] {
            fs::write(env.0.join("os-release"), format!("ID={distro}\n")).unwrap();
            fs::write(env.0.join("dnf/automatic.conf"), "[commands]\nupgrade_type = default\napply_updates = no\nreboot = never\nrandom_sleep = 42\n[emitters]\nemit_via = stdio\n[base]\ndebuglevel = 1\n").unwrap();
            for reboot in ["true", "false", "false"] {
                stdout(env.run(&p, None, Some(("reboot", reboot))));
                let config = fs::read_to_string(env.0.join("dnf/automatic.conf")).unwrap();
                assert_eq!(config.matches("upgrade_type = security").count(), 1);
                assert!(config.contains("apply_updates = yes"));
                assert!(config.contains("download_updates = yes"));
                assert!(config.contains(if reboot == "true" {
                    "reboot = when-needed"
                } else {
                    "reboot = never"
                }));
                assert!(config.contains("random_sleep = 42"));
                assert!(config.contains("[emitters]\nemit_via = stdio"));
                assert!(config.contains("[base]\ndebuglevel = 1"));
                assert!(!config.contains("upgrade_type = default"));
            }
        }
        let actions = fs::read_to_string(env.0.join("actions")).unwrap();
        assert!(actions.contains(&format!("enable --now {timer}")));
        assert!(!actions.contains("unexpected-package-operation"));
    }
    assert!(!env.0.join("systemd/shui-security-updates.service").exists());
    assert!(!env.0.join("systemd/shui-security-updates.timer").exists());
    env.stub(
        "systemctl",
        "case \"$*\" in cat*) exit 0 ;; *) exit 1 ;; esac",
    );
    assert!(!env.run(&p, None, None).status.success());
}

#[test]
fn automatic_security_updates_use_existing_native_yast_schedule() {
    let env = Sandbox::new("native-yast-security");
    for tool in ["cat", "sed", "mktemp", "cp", "mv", "rm", "readlink"] {
        env.link(tool);
    }
    for directory in ["sysconfig", "cron.daily", "systemd"] {
        fs::create_dir(env.0.join(directory)).unwrap();
    }
    fs::write(env.0.join("online_update"), "#!/bin/sh\nexit 99\n").unwrap();
    fs::set_permissions(
        env.0.join("online_update"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    symlink(
        env.0.join("online_update"),
        env.0.join("cron.daily/opensuse.org-online_update"),
    )
    .unwrap();
    env.stub("systemctl", "exit 0");
    env.stub(
        "zypper",
        "echo unexpected-package-operation >> \"$HOME/actions\"; exit 99",
    );
    fs::write(env.0.join("os-release"), "ID=sles\n").unwrap();
    fs::write(
        env.0.join("sysconfig/automatic_online_update"),
        "AOU_PATCH_CATEGORIES=\"recommended\"\nAOU_AUTO_AGREE_WITH_LICENSES=\"false\"\n",
    )
    .unwrap();
    let mut p = profile("Configure automatic security updates");
    let script = p.fields.get_mut("script").unwrap();
    for (from, to) in [
        ("/etc/os-release", "os-release"),
        ("/usr/lib/YaST2/bin/online_update", "online_update"),
        ("/etc/sysconfig", "sysconfig"),
        ("/etc/cron.", "cron."),
        ("/etc/systemd/system", "systemd"),
        ("/run/systemd/system", "systemd"),
    ] {
        *script = script.replace(from, &format!("{}/{to}", env.0.display()));
    }
    p.command = "sh -c %{script} -- %{input:reboot}".into();
    for reboot in ["false", "true", "false"] {
        stdout(env.run(&p, None, Some(("reboot", reboot))));
        let policy = fs::read_to_string(env.0.join("sysconfig/automatic_online_update")).unwrap();
        assert_eq!(policy.matches("AOU_PATCH_CATEGORIES=").count(), 1);
        assert!(policy.contains("AOU_PATCH_CATEGORIES=\"security\""));
        assert!(policy.contains("AOU_SKIP_INTERACTIVE_PATCHES=\"true\""));
        assert!(policy.contains("AOU_INCLUDE_RECOMMENDS=\"false\""));
        assert!(policy.contains("AOU_AUTO_AGREE_WITH_LICENSES=\"false\""));
        assert!(!policy.contains("recommended"));
    }
    assert!(!env.0.join("actions").exists());
    fs::remove_file(env.0.join("cron.daily/opensuse.org-online_update")).unwrap();
    let before = fs::read(env.0.join("sysconfig/automatic_online_update")).unwrap();
    let output = env.run(&p, None, None);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Update this system manually"));
    assert_eq!(
        fs::read(env.0.join("sysconfig/automatic_online_update")).unwrap(),
        before
    );
}

#[test]
fn automatic_security_updates_traverse_nested_guests_and_continue_after_failures() {
    let env = Sandbox::new("native-security-traversal");
    for tool in ["timeout", "jq", "base64"] {
        env.link(tool);
    }
    env.stub("docker", r#"
case "$1" in
  ps) case "${TARGET:-host}" in host) printf 'first\nbroken\nnoshell\nafter\n' ;; first) echo nested ;; esac ;;
  exec)
    shift 3
    target=$1; shift
    [ "$target" != noshell ] || exit 127
    TARGET=$target "$@"
    ;;
  *) exit 99 ;;
esac
"#);
    env.stub("virsh", r#"
uri=$2 action=$3
case "$action" in
  list)
    [ "${TARGET:-host}" = host ] || exit 0
    case "$uri" in lxc:///) echo lxc ;; qemu:///system) printf 'noagent\nqemu\n' ;; *) exit 99 ;; esac
    ;;
  lxc-enter-namespace)
    shift 5
    TARGET=lxc "$@"
    ;;
  qemu-agent-command)
    [ "$4" != noagent ] || exit 1
    shift 6
    action=$(printf '%s\n' "$1" | jq -r .execute)
    case "$action" in
      guest-ping) echo '{"return":{}}' ;;
      guest-exec)
        printf '%s\n' "$1" > "$HOME/request"
        command=$(printf '%s\n' "$1" | jq -r '.arguments | [.path] + .arg | @sh')
        TARGET=qemu sh -c "$command" > "$HOME/qemu-out" 2> "$HOME/qemu-err"
        echo '{"return":{"pid":42}}'
        ;;
      guest-exec-status)
        jq -nc --arg out "$(base64 < "$HOME/qemu-out")" --arg err "$(base64 < "$HOME/qemu-err")" '{return:{exited:true,exitcode:0,"out-data":$out,"err-data":$err}}'
        ;;
      *) exit 99 ;;
    esac
    ;;
  *) exit 99 ;;
esac
"#);
    let mut p = profile("Configure automatic security updates");
    p.fields.insert(
        "script".into(),
        "echo \"${TARGET:-host}:$1\" >> \"$HOME/actions\"; [ \"${TARGET:-host}\" != broken ]"
            .into(),
    );
    let traverse = "TARGET=host sh -c %{traverse} -- %{script} %{traverse} %{input:reboot}";
    p.command = format!("{traverse} 3");
    let output = env.run(&p, None, None);
    assert!(!output.status.success());
    let actions = fs::read_to_string(env.0.join("actions")).unwrap();
    for target in ["host", "first", "nested", "broken", "after", "lxc", "qemu"] {
        assert!(
            actions.contains(&format!("{target}:false\n")),
            "{target}: {actions}, {:?}",
            output.stderr
        );
    }
    assert!(!actions.contains("noshell:"));
    let warnings = String::from_utf8_lossy(&output.stderr);
    assert!(warnings.contains("Update affected systems manually"));
    assert!(warnings.contains("no accessible shell"));
    assert!(warnings.contains("no accessible guest agent"));
    assert!(
        fs::read_to_string(env.0.join("request"))
            .unwrap()
            .contains("guest-exec")
    );
    fs::remove_file(env.0.join("actions")).unwrap();
    p.fields.insert(
        "script".into(),
        "echo \"${TARGET:-host}:$1\" >> \"$HOME/actions\"".into(),
    );
    assert!(env.run(&p, None, Some(("reboot", "true"))).status.success());
    assert!(
        fs::read_to_string(env.0.join("actions"))
            .unwrap()
            .contains("qemu:true")
    );
    fs::remove_file(env.0.join("actions")).unwrap();
    p.command = format!("{traverse} 0");
    let output = env.run(&p, None, None);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("depth limit"));
    assert_eq!(
        fs::read_to_string(env.0.join("actions")).unwrap(),
        "host:false\n"
    );

    fs::remove_file(env.0.join("actions")).unwrap();
    fs::remove_file(env.0.join("bin/virsh")).unwrap();
    env.stub(
        "docker",
        r#"
case "$1" in
  ps)
    [ "${TARGET:-host}" = host ] || exit 0
    n=1; while [ "$n" -le 17 ]; do echo "guest$n"; n=$((n + 1)); done
    ;;
  exec) shift 3; target=$1; shift; TARGET=$target "$@" ;;
  *) exit 99 ;;
esac
"#,
    );
    p.command = format!("{traverse} 3");
    let output = env.run(&p, None, None);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("16-guest limit"));
    let actions = fs::read_to_string(env.0.join("actions")).unwrap();
    assert_eq!(actions.lines().count(), 17);
    assert!(!actions.contains("guest17:"));
}

#[test]
fn bundled_configuration_does_not_offer_ad_hoc_security_upgrades() {
    for windows in [false, true] {
        let config =
            Config::parse(&crate::config_defaults::defaults(windows, true).unwrap()).unwrap();
        assert!(
            !config
                .configuration_scripts
                .iter()
                .any(|p| p.name == "Install security updates now")
        );
    }
}

#[test]
fn local_log_limits_need_no_guest_protocol_or_jq() {
    let env = Sandbox::new("local-log-limits");
    for tool in [
        "bash", "timeout", "mkdir", "mktemp", "dirname", "cat", "awk", "chmod", "stat", "mv", "rm",
    ] {
        env.link(tool);
    }
    env.stub("logrotate", "exit 0");
    let mut p = profile("Configure log limits and cleanup");
    for source in p.fields.values_mut() {
        for path in ["/etc/logrotate", "/etc/libvirt", "/run/systemd/system"] {
            *source = source.replace(
                path,
                &format!("{}/{}", env.0.display(), path.rsplit('/').next().unwrap()),
            );
        }
    }
    p.fields.insert(
        "packages".into(),
        "[ \"$#\" -gt 0 ] || exit 0; echo unexpected > \"$HOME/installed\"; exit 99".into(),
    );
    p.command = "sh -c %{bootstrap} -- %{bootstrap} %{maintenance} %{packages} %{input:disk} %{input:runtime} %{input:days} %{input:file_size} %{input:backups} %{input:cleanup} %{input:all_archives} %{input:package_caches} %{input:user_caches} %{input:all_users} %{input:guests} %{input:depth}".into();
    let output = stdout(env.run(&p, None, Some(("cleanup", "false"))));
    assert!(!output.contains("SHUI_LOG_RESULT"));
    assert!(!env.0.join("installed").exists());
    assert!(
        fs::read_to_string(env.0.join("logrotate.conf"))
            .unwrap()
            .contains("maxsize 10485760")
    );
    for input in &mut p.inputs {
        match input.id.as_str() {
            "cleanup" => input.default = Some("false".into()),
            "guests" => input.default = Some("true".into()),
            "depth" => input.default = Some("0".into()),
            _ => {}
        }
    }
    let output = stdout(env.run(&p, None, None));
    assert!(!output.contains("SHUI_LOG_RESULT"));
    assert!(!env.0.join("installed").exists());

    // A depth-zero child must still return its parent's traversal budget.
    env.link("jq");
    let command = p.command.clone();
    p.command.push_str(" '{\"seen\":[],\"remaining\":7}'");
    let output = stdout(env.run(&p, None, None));
    assert!(output.contains("SHUI_LOG_RESULT="));
    assert!(output.contains("\"remaining\":7"));
    p.command = format!("{command} ''");
    assert!(!env.run(&p, None, None).status.success());
}

#[test]
fn docker_log_limits_preserve_the_effective_driver() {
    let env = Sandbox::new("docker-log-driver");
    for tool in ["bash", "jq", "cat"] {
        env.link(tool);
    }
    env.stub(
        "docker",
        "[ \"$*\" = 'info --format {{.LoggingDriver}}' ]; cat \"$HOME/driver\"",
    );
    let mut p = profile("Configure log limits and cleanup");
    let containers = p.fields["maintenance"]
        .split("container_logs() {")
        .nth(1)
        .unwrap()
        .split("libvirt_logs() {")
        .next()
        .unwrap();
    let script = format!("set -eu\nwork=\"$HOME\"\nfile_bytes=1024\nbackups=3\nwrite_config() {{ cat > \"$1\"; }}\ncontainer_logs() {{{containers}\ncontainer_logs")
        .replace("/etc/docker/daemon.json", &format!("{}/daemon.json", env.0.display()));
    p.fields.insert("test_script".into(), script);
    p.command = "bash -c %{test_script}".into();
    let config = env.0.join("daemon.json");
    fs::write(env.0.join("driver"), "journald\n").unwrap();
    stdout(env.run(&p, None, None));
    assert!(!config.exists());
    fs::write(env.0.join("driver"), "local\n").unwrap();
    stdout(env.run(&p, None, None));
    let saved = fs::read_to_string(&config).unwrap();
    assert!(!saved.contains("log-driver"));
    assert!(saved.contains("max-size") && saved.contains("1024"));
    env.stub("docker", "exit 42");
    assert!(!env.run(&p, None, None).status.success());
    assert_eq!(fs::read_to_string(&config).unwrap(), saved);
}

#[test]
fn unchecked_log_cleanup_does_not_rotate_or_vacuum_journals() {
    let env = Sandbox::new("journal-no-cleanup");
    for tool in ["bash", "cat"] {
        env.link(tool);
    }
    fs::create_dir(env.0.join("systemd")).unwrap();
    env.stub("systemctl", "printf '%s\\n' \"$*\" >> \"$HOME/actions\"");
    env.stub("journalctl", "echo unexpected > \"$HOME/cleaned\"");
    let mut p = profile("Configure log limits and cleanup");
    let journals = p.fields["maintenance"]
        .split("journals() {")
        .nth(1)
        .unwrap()
        .split("text_logs() {")
        .next()
        .unwrap();
    let source = format!("set -eu\ncleanup_logs=false disk=512M runtime=128M days=30 aggressive=0\nwrite_config() {{ cat > /dev/null; }}\njournals() {{{journals}\njournals").replace("/run/systemd/system", &format!("{}/systemd", env.0.display()));
    p.fields.insert("test_script".into(), source);
    p.command = "bash -c %{test_script}".into();
    stdout(env.run(&p, None, None));
    assert!(!env.0.join("cleaned").exists());
}

#[test]
fn archive_cleanup_keeps_open_and_unconfirmed_unknown_logs() {
    let env = Sandbox::new("archive-confirmation");
    for tool in [
        "bash", "cat", "find", "sort", "stat", "grep", "date", "sed", "rm", "mkdir",
    ] {
        env.link(tool);
    }
    for directory in ["logs", "work", "state", "proc/1/fd"] {
        fs::create_dir_all(env.0.join(directory)).unwrap();
    }
    for file in ["open.log.1", "known.log.1", "unknown.log.1"] {
        fs::write(env.0.join("logs").join(file), "log\n").unwrap();
    }
    symlink(env.0.join("logs/open.log.1"), env.0.join("proc/1/fd/7")).unwrap();
    fs::write(
        env.0.join("state/status"),
        format!(
            "logrotate state -- version 2\n\"{0}/logs/known.log\" 2026-1-1-0:0:0\n\"{0}/logs/open.log\" 2026-1-1-0:0:0\n",
            env.0.display()
        ),
    )
    .unwrap();
    let mut p = profile("Configure log limits and cleanup");
    let archives = p.fields["maintenance"]
        .split("archives() {")
        .nth(1)
        .unwrap()
        .split("cache_cleanup() {")
        .next()
        .unwrap();
    let source = format!("set -euo pipefail\ncleanup_logs=true aggressive=1 backups=3 days=30\nwork=\"$HOME/work\"\narchives() {{{archives}\narchives")
        .replace("/var/log", &format!("{}/logs", env.0.display()))
        .replace("/var/lib/logrotate", &format!("{}/state", env.0.display()))
        .replace("/proc/", &format!("{}/proc/", env.0.display()))
        .replace("/dev/tty", &format!("{}/no-terminal", env.0.display()));
    p.fields.insert("test_script".into(), source);
    p.command = "bash -c %{test_script}".into();
    let output = stdout(env.run(&p, None, None));
    assert!(env.0.join("logs/open.log.1").exists());
    assert!(env.0.join("logs/unknown.log.1").exists());
    assert!(!env.0.join("logs/known.log.1").exists());
    assert!(
        output.contains("Keeping open or unverifiable log")
            && output.contains("keeping unknown log")
    );
    // Open the unknown candidate after recognition, just before confirmation.
    fs::remove_dir_all(env.0.join("work/archive-directories")).unwrap();
    fs::write(env.0.join("no-terminal"), "y\n").unwrap();
    fs::remove_file(env.0.join("bin/grep")).unwrap();
    env.link("ln");
    env.stub("grep", "case \"$2\" in */unknown.log) ln -sf \"$HOME/logs/unknown.log.1\" \"$HOME/proc/1/fd/8\" ;; esac; exec /usr/bin/grep \"$@\"");
    let output = stdout(env.run(&p, None, None));
    assert!(output.contains("Unknown rotated log"));
    assert!(env.0.join("logs/unknown.log.1").exists());
    // An incomplete descriptor scan is never permission to delete.
    fs::remove_dir_all(env.0.join("work/archive-directories")).unwrap();
    fs::remove_dir_all(env.0.join("proc")).unwrap();
    fs::remove_file(env.0.join("bin/grep")).unwrap();
    env.link("grep");
    fs::write(env.0.join("no-terminal"), "y\n").unwrap();
    let output = stdout(env.run(&p, None, None));
    assert!(output.contains("Keeping open or unverifiable log"));
    assert!(env.0.join("logs/unknown.log.1").exists());
    // Force multiple find -execdir batches in the same directory.
    fs::remove_dir_all(env.0.join("work/archive-directories")).unwrap();
    fs::write(env.0.join("no-terminal"), "").unwrap();
    for n in 0..900 {
        fs::write(
            env.0
                .join("logs")
                .join(format!("{n:04}-{}.log.1", "x".repeat(180))),
            "log",
        )
        .unwrap();
    }
    let output = stdout(env.run(&p, None, None));
    assert_eq!(output.matches("Unknown rotated log:").count(), 901);
}

#[test]
fn podman_logging_preserves_existing_directory_permissions() {
    let env = Sandbox::new("podman-config-permissions");
    for tool in [
        "bash", "mkdir", "dirname", "mktemp", "cat", "chmod", "mv", "rm", "stat",
    ] {
        env.link(tool);
    }
    env.stub("podman", "exit 0");
    let mut p = profile("Configure log limits and cleanup");
    let script = &p.fields["maintenance"];
    let writer = script
        .split("write_config() (")
        .nth(1)
        .unwrap()
        .split("journals() {")
        .next()
        .unwrap();
    let containers = script
        .split("container_logs() {")
        .nth(1)
        .unwrap()
        .split("libvirt_logs() {")
        .next()
        .unwrap();
    let script = format!("set -eu\numask 077\nfile_bytes=1024\nwrite_config() ({writer}\ncontainer_logs() {{{containers}\ncontainer_logs")
        .replace("/etc/containers", &format!("{}/containers", env.0.display()));
    p.fields.insert("test_script".into(), script);
    p.command = "bash -c %{test_script}".into();
    let directory = env.0.join("containers/containers.conf.d");
    for existing in [false, true] {
        if existing {
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o750)).unwrap();
        }
        stdout(env.run(&p, None, None));
        for path in [env.0.join("containers"), directory.clone()] {
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                if existing && path == directory {
                    0o750
                } else {
                    0o755
                }
            );
        }
        let file = directory.join("90-shui-logging.conf");
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o644
        );
        let saved = fs::read_to_string(file).unwrap();
        assert!(saved.contains("log_size_max=1024"));
        assert!(!saved.contains("log_driver"));
    }
}

#[test]
fn shell_log_validation_rejects_invalid_limits_before_maintenance() {
    let env = Sandbox::new("log-validation");
    env.link("bash");
    env.stub("jq", "echo unexpected > \"$HOME/changed\"; exit 99");
    let mut p = profile("Configure log limits and cleanup");
    p.command = "sh -c %{bootstrap} -- %{bootstrap} %{maintenance} %{packages} %{input:disk} %{input:runtime} %{input:days} %{input:file_size} %{input:backups} %{input:cleanup} %{input:all_archives} %{input:package_caches} %{input:user_caches} %{input:all_users} %{input:guests} %{input:depth}".into();
    p.fields.insert(
        "packages".into(),
        "echo unexpected > \"$HOME/changed\"; exit 99".into(),
    );
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
    for input in &mut p.inputs {
        if input.id == "cleanup" {
            input.default = Some("false".into());
        }
    }
    for (id, message) in [
        ("all_archives", "requires rotated-log cleanup"),
        ("all_users", "requires user cache cleanup"),
    ] {
        let out = env.run(&p, None, Some((id, "true")));
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains(message));
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
        .args(["-c", "command -p -v date"])
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
    // Reject the option even when more than a pipeful of input is waiting.
    let keys = b"ssh-ed25519 AAAATEST\n".repeat(8192);
    let output = env.run_input(&p, None, None, &keys);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "Unknown key expiry."
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

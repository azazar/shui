//! Export real, validated command templates for the opt-in Docker integration suite.
use crate::{
    Target,
    launch::{Config, DEFAULT_CONFIG},
};
use std::{collections::HashSet, fs, path::PathBuf};

#[test]
#[ignore = "fixture generator for tests/docker/run.py; requires SHUI_DOCKER_FIXTURES"]
fn export_docker_fixtures() {
    let directory =
        PathBuf::from(std::env::var_os("SHUI_DOCKER_FIXTURES").expect("fixture directory"));
    fs::create_dir_all(&directory).unwrap();
    let config = Config::parse(DEFAULT_CONFIG).unwrap();
    let target = Target {
        host: "remote@shui-docker".into(),
        port: Some("2222".into()),
    };
    let covered = [
        ("SSH", "ssh"),
        ("Bash", "bash"),
        ("Tmux", "tmux"),
        ("Zellij", "zellij"),
        ("Monitor", "monitor"),
    ];
    // These need client-side transport/GUI behavior, not just remote script execution.
    let excluded = ["Mosh", "MC", "SFTP", "Tunnel", "SOCKS", "Dropbear"];
    let known: HashSet<_> = covered
        .iter()
        .map(|(name, _)| *name)
        .chain(excluded)
        .collect();
    assert_eq!(
        config.profiles.len(),
        known.len(),
        "update Docker coverage for added/removed profiles"
    );
    for p in &config.profiles {
        assert!(
            known.contains(p.name.as_str()),
            "unclassified profile: {}",
            p.name
        );
    }
    let write = |name: &str, command: (String, Vec<String>)| {
        fs::write(
            directory.join(format!("{name}.sh")),
            format!(
                "#!/bin/sh\nexec {}\n",
                shell_words::join(std::iter::once(command.0).chain(command.1))
            ),
        )
        .unwrap();
    };
    for (name, id) in covered {
        let p = config.profiles.iter().find(|p| p.name == name).unwrap();
        write(
            id,
            p.command_with_inputs(&target, &p.input_defaults()).unwrap(),
        );
        if let Some(command) = p.prepare(&target).unwrap() {
            write(&format!("{id}-prepare"), command);
        }
    }
    let recipes = [
        ("Configure key-based auth", "auth"),
        ("Install dotfiles", "dotfiles"),
        ("Configure passwordless sudo", "sudo-nopasswd"),
        ("Configure SSH-agent sudo", "sudo-agent"),
        ("Configure automatic security updates", "security-updates"),
        ("Install Docker / Podman", "containers"),
        ("Configure log limits and cleanup", "logs"),
        ("Configure firewall", "firewall"),
        ("Copy terminal definition", "terminfo"),
    ];
    assert_eq!(
        config.configuration_scripts.len(),
        recipes.len(),
        "update Docker coverage for added/removed recipes"
    );
    for (name, id) in recipes {
        let mut p = config
            .configuration_scripts
            .iter()
            .find(|p| p.name == name)
            .expect("known setup recipe")
            .clone();
        // The key generator normally discovers these on the client during the dialog.
        // Resolve its two possible choices without reading the developer's SSH directory.
        for input in &mut p.inputs {
            if input.id == "key" {
                input.command = None;
                input.options = vec!["id_ed25519.pub".into(), "Generate new Ed25519 key".into()];
                input.default = Some("id_ed25519.pub".into());
            }
        }
        let values = p.input_defaults();
        write(id, p.command_with_inputs(&target, &values).unwrap());
        if id == "containers" {
            let mut podman = values.clone();
            podman.insert("engine".into(), "Podman".into());
            write("podman", p.command_with_inputs(&target, &podman).unwrap());
        }
        if id == "logs" {
            let mut limits = values.clone();
            limits.insert("disk".into(), "16M".into());
            limits.insert("runtime".into(), "16M".into());
            limits.insert("days".into(), "2".into());
            write(
                "journald-small",
                p.command_with_inputs(&target, &limits).unwrap(),
            );
            let mut cleanup = limits.clone();
            cleanup.insert("caches".into(), "Package and login-user caches".into());
            write(
                "logs-retention",
                p.command_with_inputs(&target, &cleanup).unwrap(),
            );
            cleanup.insert(
                "cleanup".into(),
                "Remove all archives and selected caches".into(),
            );
            write(
                "logs-cleanup",
                p.command_with_inputs(&target, &cleanup).unwrap(),
            );
            cleanup.insert("guests".into(), "Running guests recursively".into());
            write(
                "logs-guests",
                p.command_with_inputs(&target, &cleanup).unwrap(),
            );
            limits.insert("disk".into(), "invalid-size".into());
            assert!(p.command_with_inputs(&target, &limits).is_err());
            // Bypass only client patterns to exercise the remote safety check too.
            let mut remote_check = p.clone();
            for input in &mut remote_check.inputs {
                input.pattern = None;
            }
            write(
                "journald-invalid",
                remote_check.command_with_inputs(&target, &limits).unwrap(),
            );
        }
        if id == "firewall" {
            for (name, ips, ports) in [
                (
                    "firewall-restricted",
                    "192.0.2.2,2001:db8::2",
                    "8080/tcp 5353/udp",
                ),
                ("firewall-changed", "192.0.2.3", "8080/tcp"),
                ("firewall-invalid-ip", "not-an-ip", ""),
                ("firewall-invalid-port", "", "70000/tcp"),
                ("firewall-ssh-bypass", "192.0.2.2", "2200-2300/tcp"),
            ] {
                let mut options = values.clone();
                options.insert("ssh_ips".into(), ips.into());
                options.insert("ports".into(), ports.into());
                let mut remote_check = p.clone();
                if matches!(name, "firewall-invalid-ip" | "firewall-invalid-port") {
                    assert!(p.command_with_inputs(&target, &options).is_err());
                    for input in &mut remote_check.inputs {
                        input.pattern = None;
                    }
                }
                write(
                    name,
                    remote_check.command_with_inputs(&target, &options).unwrap(),
                );
            }
        }
        if id == "auth" {
            for expiry in ["8 hours", "day", "week", "month", "year", "never"] {
                let mut expiring = values.clone();
                expiring.insert("expiry".into(), expiry.into());
                write(
                    &format!("auth-expiry-{}", expiry.replace(' ', "-")),
                    p.command_with_inputs(&target, &expiring).unwrap(),
                );
            }
            let mut generated = values;
            generated.insert("key".into(), "Generate new Ed25519 key".into());
            write(
                "auth-generate",
                p.command_with_inputs(&target, &generated).unwrap(),
            );
        }
    }
    fs::write(directory.join("coverage.txt"), "Profiles: SSH, Bash, Tmux, Zellij, Monitor\nRecipes: key auth (existing and generated), dotfiles install, passwordless sudo, SSH-agent sudo, terminal definition\nService recipes (--services): security updates, Docker/Podman, log limits and cleanup (including guest agents and caches), firewall\nExcluded: Mosh (UDP client), MC (VFS client), SFTP (client), Tunnel/SOCKS (client forwarding), Dropbear/PuTTY/Plink (alternative clients)\n").unwrap();
}

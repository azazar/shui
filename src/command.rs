//! Command interpreters. Direct execution preserves arguments without a shell.
use serde::Deserialize;
use std::io;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Shell {
    #[default]
    Sh,
    Direct,
    Powershell,
}

impl Shell {
    pub fn command(self, code: String) -> io::Result<(String, Vec<String>)> {
        match self {
            Self::Sh => Ok(("sh".into(), vec!["-c".into(), code])),
            Self::Direct => {
                let mut words = shell_words::split(&code)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
                if words.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "empty direct command",
                    ));
                }
                Ok((words.remove(0), words))
            }
            Self::Powershell => {
                use base64::Engine;
                let code = format!(
                    "$ErrorActionPreference = 'Stop'; $ProgressPreference = 'SilentlyContinue'; $shuiOutputEncoding = [Console]::OutputEncoding; try {{ [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); {code} }} finally {{ [Console]::OutputEncoding = $shuiOutputEncoding }}"
                );
                let bytes: Vec<_> = code.encode_utf16().flat_map(u16::to_le_bytes).collect();
                Ok((
                    "powershell.exe".into(),
                    vec![
                        "-NoLogo".into(),
                        "-NoProfile".into(),
                        "-OutputFormat".into(),
                        "Text".into(),
                        "-EncodedCommand".into(),
                        base64::engine::general_purpose::STANDARD.encode(bytes),
                    ],
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Target, config_defaults, launch::Config};

    #[test]
    fn windows_profiles_construct_literal_native_arguments() {
        let config = Config::parse(&config_defaults::defaults(true, false).unwrap()).unwrap();
        for profile in &config.configuration_scripts {
            let (program, args) = profile
                .command(&Target {
                    host: "alice@host".into(),
                    port: Some("2222".into()),
                })
                .unwrap();
            let length = program.len()
                + args
                    .iter()
                    .map(|a| a.encode_utf16().count() + 3)
                    .sum::<usize>();
            assert!(
                length < 32767,
                "{} exceeds the Windows command-line limit: {length}",
                profile.name
            );
        }
        for port in [None, Some("2222".into())] {
            let target = Target {
                host: "alice@host".into(),
                port: port.clone(),
            };
            for profile in &config.profiles {
                assert_eq!(profile.shell, Shell::Direct);
                let (program, args) = profile.command(&target).unwrap();
                assert_eq!(program, profile.program());
                assert!(!matches!(program.as_str(), "sh" | "bash" | "cmd"));
                if port.is_none() {
                    assert!(
                        !args
                            .iter()
                            .any(|a| a == "-p" || a == "-P" || a.starts_with("--ssh="))
                    );
                }
            }
        }
        let mut profile = config
            .profiles
            .iter()
            .find(|p| p.name == "Tunnel")
            .unwrap()
            .clone();
        let value = "8080:$(touch injected);'host:80";
        assert!(profile.inputs[0].validate_value(value).is_err());
        // Quoting remains safe even for custom configurations without a pattern.
        profile.inputs[0].pattern = None;
        let (_, args) = profile
            .command_with_inputs(
                &Target {
                    host: "alice@host".into(),
                    port: None,
                },
                &std::collections::HashMap::from([("forward".into(), value.into())]),
            )
            .unwrap();
        assert!(args.contains(&format!("127.0.0.1:{value}")));
    }

    #[test]
    fn powershell_uses_encoded_unicode_and_literal_substitution() {
        use base64::Engine;
        let config = Config::parse("default: Test\nprofiles:\n  - name: Test\n    shell: powershell\n    command: Write-Output %{input:value}\n    inputs: [{id: value, type: promptString, default: \"a'b $() ü\"}]\n").unwrap();
        let (program, args) = config.profiles[0]
            .command(&Target {
                host: "host".into(),
                port: None,
            })
            .unwrap();
        assert_eq!(program, "powershell.exe");
        assert_eq!(args[args.len() - 2], "-EncodedCommand");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(args.last().unwrap())
            .unwrap();
        let units: Vec<_> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let code = String::from_utf16(&units).unwrap();
        assert!(code.contains("Write-Output 'a''b $() ü'"), "{code}");
    }
}

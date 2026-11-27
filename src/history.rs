//! Match the client command that a profile writes, without running it.
use crate::{
    Target,
    launch::{Profile, history_words},
    parse_port, target,
};
use regex::Regex;
use std::{collections::HashMap, io};

#[derive(Debug, Default)]
pub struct Patterns(HashMap<String, Vec<Pattern>>);

#[derive(Debug)]
struct Pattern {
    profile: String,
    regex: Regex,
    captures: Vec<Field>,
}

#[derive(Debug)]
enum Field {
    Host,
    Port,
}

// NUL cannot occur in configured commands or their arguments.
const HOST: &str = "\0host\0";
const PORT: &str = "\0port\0";

impl Patterns {
    pub fn new(profiles: &[Profile]) -> io::Result<Self> {
        let mut patterns = Self::default();
        for profile in profiles.iter().filter(|p| p.inputs.is_empty()) {
            for port in [Some(PORT), None] {
                let sample = Target {
                    host: HOST.into(),
                    port: port.map(str::to_owned),
                };
                let Ok(line) = profile.usage_line(&sample) else {
                    continue;
                };
                if line
                    .replace(HOST, "")
                    .replace(PORT, "")
                    .chars()
                    .any(char::is_control)
                {
                    continue;
                }
                let Some(words) = history_words(&line) else {
                    continue;
                };
                let Some(program) = words.first() else {
                    continue;
                };
                // Match argv, so equivalent Bash quoting (including IPv6 brackets)
                // does not change the destination or profile.
                let template = words.join("\n");
                if !template.contains(HOST) || (port.is_some() && !template.contains(PORT)) {
                    continue;
                }
                let pattern = regex::escape(&template)
                    .replace(HOST, r"([^\n]+)")
                    .replace(PORT, "([0-9]+)");
                let captures = template
                    .split('\0')
                    .skip(1)
                    .step_by(2)
                    .map(|slot| {
                        if slot == "host" {
                            Field::Host
                        } else {
                            Field::Port
                        }
                    })
                    .collect();
                let regex = Regex::new(&format!(r"\A{pattern}\z")).map_err(|e| {
                    io::Error::other(format!("{} history pattern: {e}", profile.name))
                })?;
                patterns
                    .0
                    .entry(program.clone())
                    .or_default()
                    .push(Pattern {
                        profile: profile.name.clone(),
                        regex,
                        captures,
                    });
            }
        }
        Ok(patterns)
    }

    pub fn find(&self, words: &[String], user: &str) -> Option<(Target, &str)> {
        let patterns = self.0.get(words.first()?)?;
        let line = words.join("\n");
        patterns.iter().find_map(|pattern| {
            let captures = pattern.regex.captures(&line)?;
            // Repeated %h/%p/%t must describe the same connection throughout.
            let mut host = None;
            let mut port = None;
            for (value, field) in captures.iter().skip(1).zip(&pattern.captures) {
                let slot = match field {
                    Field::Host => &mut host,
                    Field::Port => &mut port,
                };
                let value = value?.as_str();
                if slot.replace(value).is_some_and(|old| old != value) {
                    return None;
                }
            }
            let mut destination = target(host?, user)?;
            if let Some(port) = port {
                destination.port = Some(parse_port(port)?.into());
            }
            Some((destination, pattern.profile.as_str()))
        })
    }
}

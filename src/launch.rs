use crate::Target;
use crate::command::Shell;
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

pub fn config_path(home: &Path) -> PathBuf {
    config_path_with_base(
        home,
        std::env::var_os("XDG_CONFIG_HOME")
            .as_deref()
            .map(Path::new),
    )
}

fn config_path_with_base(home: &Path, base: Option<&Path>) -> PathBuf {
    base.filter(|path| path.is_absolute())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(".config"))
        .join("shui.yml")
}
pub const DEFAULT_CONFIG: &str = include_str!(concat!(env!("OUT_DIR"), "/config.yml"));
const HISTORY_TARGET: &str = " # shui-target=";
const LEGACY_HISTORY_TARGET: &str = " # ssh-menu-target=";
const SCRIPT_PREFIX: &str = "exec sh -c \"$(printf '%b' ";
const SCRIPT_SUFFIX: &str = ")\"";
const BACKGROUND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONFIG_SIZE: usize = 1024 * 1024;

pub fn read_config(path: &Path) -> io::Result<String> {
    if !fs::metadata(path)?.is_file() {
        return Err(invalid("configuration must be a regular file"));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Avoid blocking if a regular path is replaced with a FIFO after stat.
        options.custom_flags(nix::fcntl::OFlag::O_NONBLOCK.bits());
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("configuration must be a regular file"));
    }
    let mut text = String::new();
    file.take(MAX_CONFIG_SIZE as u64 + 1)
        .read_to_string(&mut text)?;
    if text.len() > MAX_CONFIG_SIZE {
        return Err(invalid("configuration exceeds 1 MiB"));
    }
    Ok(text)
}

// Checks and generators run while the UI cannot accept input. Bound both their
// lifetime and memory use, and keep pipe reads nonblocking even if a subprocess
// inherits a pipe and outlives its shell.
mod background {
    use std::{
        io::{self, Read},
        process::{Child, Command, Output, Stdio},
        time::{Duration, Instant},
    };

    const OUTPUT_LIMIT: usize = 64 * 1024;

    #[cfg(windows)]
    fn resume_child(pid: u32) -> io::Result<()> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::{
            Foundation::INVALID_HANDLE_VALUE,
            System::{
                Diagnostics::ToolHelp::{
                    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                    Thread32Next,
                },
                Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
            },
        };
        // SAFETY: snapshot and thread handles are checked and owned; entry size
        // is initialized before enumeration. Only our suspended child is resumed.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let snapshot = OwnedHandle::from_raw_handle(snapshot);
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of_val(&entry) as u32;
            let mut found = Thread32First(snapshot.as_raw_handle(), &mut entry);
            while found != 0 {
                if entry.th32OwnerProcessID == pid {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if thread.is_null() {
                        return Err(io::Error::last_os_error());
                    }
                    let thread = OwnedHandle::from_raw_handle(thread);
                    if ResumeThread(thread.as_raw_handle()) == u32::MAX {
                        return Err(io::Error::last_os_error());
                    }
                    return Ok(());
                }
                found = Thread32Next(snapshot.as_raw_handle(), &mut entry);
            }
        }
        Err(io::Error::other("suspended child thread not found"))
    }

    #[cfg(windows)]
    fn create_job() -> io::Result<std::os::windows::io::OwnedHandle> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        // SAFETY: default security, unnamed job; handle ownership transfers
        // immediately. The limit structure is initialized and correctly sized.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = OwnedHandle::from_raw_handle(handle);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }
    }

    struct Process {
        child: Child,
        stopped: bool,
        #[cfg(windows)]
        job: std::os::windows::io::OwnedHandle,
    }

    impl Process {
        fn stop(&mut self) {
            if self.stopped {
                return;
            }
            self.stopped = true;
            #[cfg(windows)]
            {
                use std::os::windows::io::AsRawHandle;
                // SAFETY: job is owned and live until Process is dropped.
                unsafe {
                    windows_sys::Win32::System::JobObjects::TerminateJobObject(
                        self.job.as_raw_handle(),
                        1,
                    );
                }
            }
            #[cfg(unix)]
            {
                use nix::{
                    sys::signal::{Signal, killpg},
                    unistd::Pid,
                };
                // This child created its own session before exec. Killing
                // the group also stops ordinary grandchildren such as sleep.
                if let Ok(pid) = i32::try_from(self.child.id()) {
                    let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
                }
            }
            let _ = self.child.kill();
        }
    }

    impl Drop for Process {
        fn drop(&mut self) {
            self.stop();
            let _ = self.child.wait();
        }
    }

    fn append(bytes: &[u8], output: &mut Vec<u8>, stream: &str) -> io::Result<()> {
        if bytes.len() > OUTPUT_LIMIT.saturating_sub(output.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{stream} exceeds 64 KiB"),
            ));
        }
        output.extend_from_slice(bytes);
        Ok(())
    }

    #[cfg(unix)]
    fn nonblocking(pipe: &impl std::os::fd::AsFd) -> io::Result<()> {
        use nix::fcntl::{FcntlArg, OFlag, fcntl};
        let flags = OFlag::from_bits_truncate(fcntl(pipe, FcntlArg::F_GETFL)?);
        fcntl(pipe, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
        Ok(())
    }

    #[cfg(unix)]
    fn drain(pipe: &mut impl Read, output: &mut Vec<u8>, stream: &str) -> io::Result<()> {
        let mut buffer = [0; 4096];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) => return Ok(()),
                Ok(count) => append(&buffer[..count], output, stream)?,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(error),
            }
        }
    }

    #[cfg(windows)]
    fn drain(
        pipe: &mut (impl Read + std::os::windows::io::AsRawHandle),
        output: &mut Vec<u8>,
        stream: &str,
    ) -> io::Result<()> {
        use windows_sys::Win32::{Foundation::ERROR_BROKEN_PIPE, System::Pipes::PeekNamedPipe};
        let mut buffer = [0; 4096];
        loop {
            let mut available = 0;
            // SAFETY: the pipe owns this live handle, the byte-count pointer is
            // valid, and null pointers request no data copy from PeekNamedPipe.
            let success = unsafe {
                PeekNamedPipe(
                    pipe.as_raw_handle(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut available,
                    std::ptr::null_mut(),
                )
            };
            if success == 0 {
                let error = io::Error::last_os_error();
                return if error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
                    Ok(())
                } else {
                    Err(error)
                };
            }
            if available == 0 {
                return Ok(());
            }
            let count = usize::try_from(available)
                .unwrap_or(usize::MAX)
                .min(buffer.len());
            match pipe.read(&mut buffer[..count]) {
                Ok(0) => return Ok(()),
                Ok(count) => append(&buffer[..count], output, stream)?,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
    }

    #[cfg(test)]
    pub(super) fn run(command: &str, capture: bool, timeout: Duration) -> io::Result<Output> {
        run_with(super::Shell::Sh, command, capture, timeout)
    }

    pub(super) fn run_with(
        shell: super::Shell,
        command: &str,
        capture: bool,
        timeout: Duration,
    ) -> io::Result<Output> {
        let (program, args) = shell.command(command.into())?;
        let mut command_builder = Command::new(program);
        command_builder.args(args).stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Assign the child to its cleanup job before any user code runs.
            command_builder.creation_flags(
                windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
                    | windows_sys::Win32::System::Threading::CREATE_SUSPENDED,
            );
        }
        if capture {
            command_builder
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
        } else {
            command_builder.stdout(Stdio::null()).stderr(Stdio::null());
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: setsid is async-signal-safe and this closure performs no
            // allocation or locking. A fresh session also prevents commands
            // such as ssh from prompting through the UI's controlling tty.
            unsafe {
                command_builder
                    .pre_exec(|| nix::unistd::setsid().map(|_| ()).map_err(io::Error::from));
            }
        }
        let mut process = Process {
            #[cfg(windows)]
            job: create_job()?,
            child: command_builder.spawn()?,
            stopped: false,
        };
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            // SAFETY: both handles are owned by process and remain live.
            if unsafe {
                windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(
                    process.job.as_raw_handle(),
                    process.child.as_raw_handle(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            resume_child(process.child.id())?;
        }
        let mut stdout = process.child.stdout.take();
        let mut stderr = process.child.stderr.take();
        #[cfg(unix)]
        {
            if let Some(pipe) = &stdout {
                nonblocking(pipe)?;
            }
            if let Some(pipe) = &stderr {
                nonblocking(pipe)?;
            }
        }
        let mut output = Vec::new();
        let mut errors = Vec::new();
        let started = Instant::now();
        loop {
            if let Some(pipe) = &mut stdout {
                drain(pipe, &mut output, "stdout")?;
            }
            if let Some(pipe) = &mut stderr {
                drain(pipe, &mut errors, "stderr")?;
            }
            if let Some(status) = process.child.try_wait()? {
                process.stop();
                if let Some(pipe) = &mut stdout {
                    drain(pipe, &mut output, "stdout")?;
                }
                if let Some(pipe) = &mut stderr {
                    drain(pipe, &mut errors, "stderr")?;
                }
                return Ok(Output {
                    status,
                    stdout: output,
                    stderr: errors,
                });
            }
            if started.elapsed() >= timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out after {} seconds", timeout.as_secs_f32()),
                ));
            }
            std::thread::sleep(
                Duration::from_millis(10).min(timeout.saturating_sub(started.elapsed())),
            );
        }
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub enum InputType {
    #[serde(rename = "promptString")]
    PromptString,
    #[serde(rename = "pickString")]
    PickString,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: InputType,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub required: bool,
    /// A Rust regular expression matched against the entire nonempty value.
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub validation_message: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub shell: Shell,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneratedInput {
    description: Option<String>,
    default: Option<String>,
    options: Option<Vec<String>>,
}

impl Input {
    pub fn resolve(&self) -> io::Result<Self> {
        let Some(command) = &self.command else {
            return Ok(self.clone());
        };
        let output = background::run_with(self.shell, command, true, BACKGROUND_TIMEOUT).map_err(
            |error| io::Error::new(error.kind(), format!("{}: generator {error}", self.label())),
        )?;
        if !output.status.success() {
            let message: String = String::from_utf8_lossy(&output.stderr)
                .chars()
                .filter(|c| !c.is_control())
                .take(200)
                .collect();
            return Err(invalid(format!(
                "{}: generator failed ({}): {}",
                self.label(),
                output.status,
                message
            )));
        }
        let generated: GeneratedInput = serde_yaml_ng::from_slice(&output.stdout)
            .map_err(|e| invalid(format!("{}: invalid generated input: {e}", self.label())))?;
        let mut resolved = self.clone();
        resolved.command = None;
        if let Some(value) = generated.description {
            resolved.description = value;
        }
        if let Some(value) = generated.default {
            resolved.default = Some(value);
        }
        if let Some(value) = generated.options {
            resolved.options = value;
        }
        resolved.validate()?;
        Ok(resolved)
    }
    pub fn initial(&self) -> String {
        self.default
            .clone()
            .unwrap_or_else(|| self.options.first().cloned().unwrap_or_default())
    }
    pub fn label(&self) -> &str {
        if self.description.is_empty() {
            &self.id
        } else {
            &self.description
        }
    }
    fn value_pattern(&self) -> io::Result<Option<regex::Regex>> {
        self.pattern
            .as_ref()
            .map(|pattern| {
                regex::Regex::new(&format!(r"\A(?:{pattern})\z")).map_err(|error| {
                    invalid(format!("{}: invalid input pattern: {error}", self.label()))
                })
            })
            .transpose()
    }
    pub fn validate_value(&self, value: &str) -> io::Result<()> {
        if self.required && value.trim().is_empty() {
            return Err(invalid(format!("{} is required", self.label())));
        }
        if value.chars().any(char::is_control)
            || (self.kind == InputType::PickString && !self.options.contains(&value.to_string()))
        {
            return Err(invalid(format!("invalid value for {}", self.label())));
        }
        // Optional empty fields bypass the pattern; required fields do not.
        if !value.is_empty()
            && self
                .value_pattern()?
                .is_some_and(|pattern| !pattern.is_match(value))
        {
            return Err(invalid(format!(
                "{}: {}",
                self.label(),
                self.validation_message
                    .as_deref()
                    .unwrap_or("value does not match the required format")
            )));
        }
        Ok(())
    }
    fn validate(&self) -> io::Result<()> {
        if self.id.is_empty()
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
        {
            return Err(invalid(
                "input id must contain only letters, digits, _ or -",
            ));
        }
        if self.description.chars().any(char::is_control)
            || self.validation_message.as_ref().is_some_and(|message| {
                message.trim().is_empty() || message.chars().any(char::is_control)
            })
            || self
                .default
                .iter()
                .chain(&self.options)
                .any(|s| s.chars().any(char::is_control))
        {
            return Err(invalid(
                "input labels and values must not contain control characters",
            ));
        }
        if self
            .command
            .as_ref()
            .is_some_and(|c| c.trim().is_empty() || c.contains('\0'))
        {
            return Err(invalid("input command must be nonempty and contain no NUL"));
        }
        match self.kind {
            InputType::PickString
                if self.command.is_none()
                    && (self.options.is_empty()
                        || self
                            .default
                            .as_ref()
                            .is_some_and(|v| !self.options.contains(v))) =>
            {
                return Err(invalid(
                    "pickString needs options and its default must be one of them",
                ));
            }
            InputType::PromptString if !self.options.is_empty() => {
                return Err(invalid("options are only supported for pickString"));
            }
            _ => {}
        }
        self.value_pattern()?;
        if self.command.is_none() {
            for value in self.default.iter().chain(&self.options) {
                // Empty prompt defaults are allowed even for required fields.
                if !value.is_empty() {
                    self.validate_value(value)?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Profile {
    pub name: String,
    #[serde(default)]
    pub key: Option<crate::shortcut::Shortcut>,
    #[serde(default)]
    pub available_command: Option<String>,
    pub command: String,
    #[serde(default)]
    pub shell: Shell,
    #[serde(default)]
    pub port_args: Vec<String>,
    #[serde(default)]
    pub prepare_command: Option<String>,
    #[serde(default)]
    pub prepare_shell: Option<Shell>,
    #[serde(default)]
    pub inputs: Vec<Input>,
    #[serde(flatten)]
    pub fields: HashMap<String, String>,
}
impl Profile {
    pub fn is_available(&self) -> bool {
        let Some(command) = &self.available_command else {
            return true;
        };
        background::run_with(self.shell, command, false, BACKGROUND_TIMEOUT)
            .is_ok_and(|output| output.status.success())
    }
    fn expand(
        &self,
        text: &str,
        target: &Target,
        stack: &mut Vec<String>,
        short: bool,
        inputs: &HashMap<String, String>,
    ) -> io::Result<String> {
        if text.len() > 65536 {
            return Err(invalid("expanded command exceeds 64 KiB"));
        }
        // Leaf fields (notably scripts) are literal: shell %, $, quotes, etc. are preserved.
        if !short && !text.contains("%{") {
            return Ok(text.into());
        }
        let substitutions = text.contains("%{")
            || (short && ["%h", "%p", "%t"].iter().any(|token| text.contains(token)));
        // Shell word quoting does not protect data inside a here-document,
        // nested command/parameter expansion, or legacy backtick substitution. Keep literal
        // scripts valid, but reject these ambiguous contexts in templates.
        if substitutions
            && ["<<", "$(", "$[", "`"]
                .iter()
                .any(|token| text.contains(token))
        {
            return Err(invalid(
                "templates with placeholders must not contain here-documents, command substitutions, arithmetic expansions, or backticks; pass values as positional arguments to a literal script",
            ));
        }
        let mut result = String::new();
        let mut chars = text.chars().peekable();
        let mut quote = None;
        while let Some(c) = chars.next() {
            if c == '\\' && quote != Some('\'') && !(short && self.shell == Shell::Powershell) {
                result.push(c);
                if let Some(next) = chars.next() {
                    result.push(next);
                }
                continue;
            }
            if substitutions && c == '$' && quote != Some('\'') && chars.peek() == Some(&'{') {
                // Parameter expansions have independent quoting rules. Keep
                // ordinary expressions such as ${port:+-p "$port"} literal;
                // interpolating inside them is deliberately unsupported.
                chars.next();
                result.push_str("${");
                let mut depth = 1;
                let mut inner_quote = None;
                while depth != 0 {
                    let part = chars
                        .next()
                        .ok_or_else(|| invalid("unclosed shell parameter expansion"))?;
                    if part == '%'
                        && (chars.peek() == Some(&'{')
                            || (short && matches!(chars.peek(), Some('h' | 'p' | 't'))))
                    {
                        return Err(invalid(
                            "placeholders must not appear inside shell parameter expansions; pass values as positional arguments to a literal script",
                        ));
                    }
                    // Fail closed on ambiguous braces/escapes rather than
                    // trying to implement the shell's entire expansion grammar.
                    if part == '\\' || (matches!(part, '{' | '}') && inner_quote.is_some()) {
                        return Err(invalid(
                            "complex shell parameter expansions must be placed in a literal script field",
                        ));
                    }
                    if matches!(part, '\'' | '"') {
                        if inner_quote == Some(part) {
                            inner_quote = None;
                        } else if inner_quote.is_none() {
                            inner_quote = Some(part);
                        }
                    }
                    match part {
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        _ => {}
                    }
                    result.push(part);
                }
                continue;
            }
            if matches!(c, '\'' | '"') {
                if quote == Some(c) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(c);
                }
            }
            if c != '%' {
                result.push(c);
                continue;
            }
            let value = match chars.peek().copied() {
                Some('%') => {
                    chars.next();
                    result.push('%');
                    continue;
                }
                Some('h' | 'p' | 't') if short => match chars.next().unwrap() {
                    'h' => target.host.clone(),
                    'p' => target.port.clone().unwrap_or_default(),
                    _ => target.label(),
                },
                Some('{') => {
                    chars.next();
                    let mut key = String::new();
                    loop {
                        match chars.next() {
                            Some('}') => break,
                            Some(c) => key.push(c),
                            None => return Err(invalid("unclosed %{field} placeholder")),
                        }
                    }
                    if let Some(id) = key.strip_prefix("input:") {
                        inputs
                            .get(id)
                            .ok_or_else(|| invalid(format!("unknown input: {id}")))?
                            .clone()
                    } else {
                        if stack.len() >= 16 || stack.contains(&key) {
                            return Err(invalid(format!(
                                "cyclic or too deeply nested field: {key}"
                            )));
                        }
                        let field = self
                            .fields
                            .get(&key)
                            .ok_or_else(|| invalid(format!("unknown field: {key}")))?;
                        stack.push(key);
                        let value = self.expand(field, target, stack, false, inputs)?;
                        stack.pop();
                        value
                    }
                }
                _ => {
                    result.push('%');
                    continue;
                }
            };
            if quote.is_some() {
                return Err(invalid(
                    "placeholders must be unquoted; they are shell-escaped automatically",
                ));
            }
            if short && self.shell == Shell::Powershell {
                result.push_str(&format!("'{}'", value.replace('\'', "''")));
            } else {
                result.push_str(&shell_words::quote(&value));
            }
            if result.len() > 65536 {
                return Err(invalid("expanded command exceeds 64 KiB"));
            }
        }
        if short && quote.is_some() {
            return Err(invalid("unclosed quote in command"));
        }
        if result.len() > 65536 {
            return Err(invalid("expanded command exceeds 64 KiB"));
        }
        Ok(result)
    }
    pub fn expanded(&self, target: &Target) -> io::Result<String> {
        self.expand(
            &self.command,
            target,
            &mut Vec::new(),
            true,
            &self.input_defaults(),
        )
    }
    pub fn input_defaults(&self) -> HashMap<String, String> {
        self.inputs
            .iter()
            .map(|i| (i.id.clone(), i.initial()))
            .collect()
    }
    pub fn command_with_inputs(
        &self,
        target: &Target,
        values: &HashMap<String, String>,
    ) -> io::Result<(String, Vec<String>)> {
        for input in &self.inputs {
            let value = values
                .get(&input.id)
                .ok_or_else(|| invalid(format!("missing input: {}", input.id)))?;
            input.validate_value(value)?;
        }
        self.build_command(
            self.expand(&self.command, target, &mut Vec::new(), true, values)?,
            target,
        )
    }
    pub fn prepare(&self, target: &Target) -> io::Result<Option<(String, Vec<String>)>> {
        self.prepare_command
            .as_ref()
            .map(|command| {
                let mut preparer = self.clone();
                preparer.shell = self.prepare_shell.unwrap_or(self.shell);
                preparer
                    .expand(command, target, &mut Vec::new(), true, &HashMap::new())
                    .and_then(|code| preparer.shell.command(code))
            })
            .transpose()
    }
    pub fn program(&self) -> String {
        let words = shell_words::split(&self.command).unwrap_or_default();
        let i = words.iter().position(|s| s == "exec").map_or(0, |i| i + 1);
        words.get(i).cloned().unwrap_or_default()
    }
    pub fn normalize_target(&self, target: &Target, users: &HashMap<String, String>) -> Target {
        // These clients do not apply OpenSSH's User directives. An explicit
        // username must not be dropped based on another client's configuration.
        if matches!(
            self.program().as_str(),
            "dbclient" | "putty" | "putty.exe" | "plink" | "plink.exe"
        ) {
            target.clone()
        } else {
            crate::ssh_config::normalize_target(target, users)
        }
    }
    pub fn command(&self, target: &Target) -> io::Result<(String, Vec<String>)> {
        self.build_command(self.expanded(target)?, target)
    }
    fn build_command(&self, code: String, target: &Target) -> io::Result<(String, Vec<String>)> {
        if self.shell == Shell::Sh {
            return Ok(Self::shell_command(code));
        }
        let (program, mut args) = self.shell.command(code)?;
        if let Some(port) = &target.port {
            let port_args = self.port_args.iter().map(|arg| arg.replace("%p", port));
            args.splice(0..0, port_args);
        }
        Ok((program, args))
    }
    fn shell_command(command: String) -> (String, Vec<String>) {
        // Keep scripts on one history line without sending the script through terminal stdin.
        let code = if command.contains(['\n', '\r']) {
            // Older /bin/sh implementations can reparse quotes and braces in
            // nested command substitutions. An octal payload has no shell
            // syntax and preserves every UTF-8 byte through printf %b.
            let escaped: String = command
                .bytes()
                .map(|byte| format!("\\0{byte:03o}"))
                .collect();
            format!(
                "{SCRIPT_PREFIX}{}{SCRIPT_SUFFIX}",
                shell_words::quote(&escaped)
            )
        } else {
            command
        };
        ("sh".into(), vec!["-c".into(), code])
    }
    pub fn history_line(&self, target: &Target) -> io::Result<String> {
        let (program, args) = self.command(target)?;
        // Preserve the existing single-line history encoding for multiline
        // arguments. This is a record only: native launches still bypass sh.
        let mut code = shell_words::join(std::iter::once(program).chain(args));
        if code.contains(['\n', '\r']) {
            let (program, args) = Self::shell_command(code);
            code = shell_words::join(std::iter::once(program).chain(args));
        }
        Ok(format!("exec {code}{HISTORY_TARGET}{}", target.label()))
    }
}

pub fn history_target_hint(line: &str) -> Option<&str> {
    split_history_hint(line).map(|(_, target)| target)
}

fn split_history_hint(line: &str) -> Option<(&str, &str)> {
    line.rsplit_once(HISTORY_TARGET)
        .or_else(|| line.rsplit_once(LEGACY_HISTORY_TARGET))
}

pub fn history_words(line: &str) -> Option<Vec<String>> {
    let mut code = split_history_hint(line)
        .map_or(line, |(command, _)| command)
        .to_owned();
    for _ in 0..4 {
        let mut words = shell_words::split(&code).ok()?;
        if words.first().is_some_and(|s| s == "exec") {
            words.remove(0);
        }
        if words.len() == 3 && words[0] == "sh" && words[1] == "-c" {
            code = words[2].clone();
            if let Some(encoded) = code
                .strip_prefix(SCRIPT_PREFIX)
                .and_then(|s| s.strip_suffix(SCRIPT_SUFFIX))
            {
                let pieces = shell_words::split(encoded).ok()?;
                if pieces.len() != 1 {
                    return None;
                }
                let mut chars = pieces[0].chars();
                let mut decoded = Vec::new();
                while let Some(c) = chars.next() {
                    if c != '\\' {
                        let mut buffer = [0; 4];
                        decoded.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
                        continue;
                    }
                    decoded.push(match chars.next()? {
                        'n' => b'\n',
                        'r' => b'\r',
                        '\\' => b'\\',
                        '0' => {
                            let digits: String = chars.by_ref().take(3).collect();
                            if digits.len() != 3 {
                                return None;
                            }
                            u8::from_str_radix(&digits, 8).ok()?
                        }
                        _ => return None,
                    });
                }
                code = String::from_utf8(decoded).ok()?;
            }
        } else {
            return Some(words);
        }
    }
    None
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UsernameDisplay {
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub default: String,
    #[serde(default)]
    pub username_display: UsernameDisplay,
    #[serde(default)]
    pub keybindings: crate::bindings::Bindings,
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub configuration_scripts: Vec<Profile>,
}
impl Config {
    pub fn parse(text: &str) -> io::Result<Self> {
        if text.len() > MAX_CONFIG_SIZE {
            return Err(invalid("configuration exceeds 1 MiB"));
        }
        let config: Self = serde_yaml_ng::from_str(text).map_err(|e| invalid(e.to_string()))?;
        config
            .keybindings
            .validate(&config.profiles)
            .map_err(invalid)?;
        if config.profiles.is_empty() {
            return Err(invalid("profiles must not be empty"));
        }
        if config
            .configuration_scripts
            .iter()
            .any(|p| p.key.is_some() || p.prepare_command.is_some())
        {
            return Err(invalid(
                "key and prepare_command are supported only in launch profiles",
            ));
        }
        let mut keys = HashSet::new();
        for profile in &config.profiles {
            if let Some(key) = &profile.key
                && !keys.insert(key)
            {
                return Err(invalid(format!("duplicate profile key: {key}")));
            }
        }
        for group in [&config.profiles, &config.configuration_scripts] {
            let mut names = HashSet::new();
            for profile in group {
                if profile
                    .available_command
                    .as_ref()
                    .is_some_and(|c| c.trim().is_empty() || c.contains('\0'))
                {
                    return Err(invalid(
                        "available_command must be nonempty and contain no NUL",
                    ));
                }
                if profile
                    .prepare_command
                    .as_ref()
                    .is_some_and(|c| c.trim().is_empty() || c.contains('\0'))
                {
                    return Err(invalid(
                        "prepare_command must be nonempty and contain no NUL",
                    ));
                }
                if (!profile.port_args.is_empty() && profile.shell != Shell::Direct)
                    || profile.port_args.iter().any(|arg| arg.contains('\0'))
                {
                    return Err(invalid(
                        "port_args require shell: direct and must not contain NUL",
                    ));
                }
                let mut ids = HashSet::new();
                for input in &profile.inputs {
                    input.validate()?;
                    if !ids.insert(&input.id) {
                        return Err(invalid(format!("duplicate input id: {}", input.id)));
                    }
                }
                if profile.name.trim().is_empty()
                    || profile.name.chars().any(char::is_control)
                    || !names.insert(&profile.name)
                {
                    return Err(invalid(
                        "profile names must be nonempty, unique and contain no control characters",
                    ));
                }
                if profile.command.trim().is_empty()
                    || profile.command.contains('\0')
                    || profile.fields.values().any(|v| v.contains('\0'))
                {
                    return Err(invalid(
                        "command must be nonempty; fields must not contain NUL",
                    ));
                }
                for port in [None, Some("2222".into())] {
                    profile.prepare(&Target {
                        host: "user@host".into(),
                        port: port.clone(),
                    })?;
                    profile
                        .expanded(&Target {
                            host: "user@host".into(),
                            port,
                        })
                        .map_err(|e| invalid(format!("profile {}: {e}", profile.name)))?;
                }
            }
        }
        if !config.profiles.iter().any(|p| p.name == config.default) {
            return Err(invalid("default must name an existing profile"));
        }
        Ok(config)
    }
    fn available_profiles(mut self) -> io::Result<Self> {
        let mut checks = HashMap::new();
        self.profiles.retain(|profile| {
            profile.available_command.as_ref().is_none_or(|command| {
                *checks
                    .entry((profile.shell, command.clone()))
                    .or_insert_with(|| profile.is_available())
            })
        });
        let first = self.profiles.first().ok_or_else(|| {
            invalid("no launch profiles are available; check available_command and installed tools")
        })?;
        if !self
            .profiles
            .iter()
            .any(|profile| profile.name == self.default)
        {
            self.default = first.name.clone();
        }
        Ok(self)
    }
    pub fn default_index(&self) -> usize {
        self.profiles
            .iter()
            .position(|p| p.name == self.default)
            .expect("validated default")
    }
    pub fn toggle_transport(&self, current: usize) -> usize {
        let program = if self.profiles[current].program() == "mosh" {
            "ssh"
        } else {
            "mosh"
        };
        self.profiles
            .iter()
            .position(|p| {
                p.program() == program
                    && !p.fields.contains_key("remote")
                    && p.command.trim_end().ends_with("%h")
            })
            .unwrap_or(current)
    }
    #[cfg(test)]
    pub fn history_profiles(&self, targets: &[Target], history: &str, user: &str) -> Vec<usize> {
        self.history_profiles_with_users(targets, history, user, &HashMap::new())
    }
    pub fn history_profiles_with_users(
        &self,
        targets: &[Target],
        history: &str,
        user: &str,
        users: &HashMap<String, String>,
    ) -> Vec<usize> {
        let mut selected = vec![self.default_index(); targets.len()];
        let indices: HashMap<_, _> = targets
            .iter()
            .enumerate()
            .map(|(i, t)| (t.label(), i))
            .collect();
        let mut invocations: HashMap<Vec<String>, Option<(usize, usize)>> = HashMap::new();
        for (i, target) in targets.iter().enumerate() {
            let mut variants = vec![target.clone()];
            if !target.host.contains('@')
                && let Some(login) = users.get(&target.host)
            {
                variants.push(Target {
                    host: format!("{login}@{}", target.host),
                    port: target.port.clone(),
                });
            }
            for target in &variants {
                for (p, profile) in self.profiles.iter().enumerate() {
                    let Some(words) = profile
                        .history_line(target)
                        .ok()
                        .and_then(|line| history_words(&line))
                    else {
                        continue;
                    };
                    invocations
                        .entry(words)
                        .and_modify(|v| {
                            if v.is_some_and(|(host, _)| host != i) {
                                *v = None;
                            }
                        })
                        .or_insert(Some((i, p)));
                }
            }
        }
        for line in history.lines() {
            let Some(words) = history_words(line) else {
                continue;
            };
            if let Some(Some((i, p))) = invocations.get(&words) {
                selected[*i] = *p;
                continue;
            }
            if let Some(hint) = history_target_hint(line) {
                if let Some(target) = crate::target(hint, user) {
                    let target = crate::ssh_config::normalize_target(&target, users);
                    if let Some(i) = indices.get(&target.label()) {
                        selected[*i] = self.default_index();
                    }
                }
                continue;
            }
            let Some((target, remote, program)) = crate::history_connection(line, user) else {
                continue;
            };
            let target = crate::ssh_config::normalize_target(&target, users);
            let Some(i) = indices.get(&target.label()) else {
                continue;
            };
            if !matches!(program.as_str(), "ssh" | "mosh") {
                continue;
            }
            selected[*i] = self
                .profiles
                .iter()
                .position(|profile| {
                    if profile.program() != program {
                        return false;
                    }
                    if remote.is_empty() {
                        return !profile.fields.contains_key("remote")
                            && profile.command.trim_end().ends_with("%h");
                    }
                    // Conventional named remote command supports ordinary manually entered SSH history.
                    profile.fields.get("remote").is_some_and(|field| {
                        profile
                            .expand(field, &target, &mut Vec::new(), false, &HashMap::new())
                            .is_ok_and(|expected| remote.join(" ") == expected)
                    })
                })
                .unwrap_or(self.default_index());
        }
        selected
    }
    pub fn load(home: &Path) -> io::Result<Self> {
        Self::load_path(&config_path(home))
    }
    fn load_path(path: &Path) -> io::Result<Self> {
        let load = || -> io::Result<Self> {
            let mut directory = fs::DirBuilder::new();
            directory.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                directory.mode(0o700);
            }
            directory.create(path.parent().expect("config parent directory"))?;
            let text = match read_config(path) {
                Ok(text) => text,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    let mut options = fs::OpenOptions::new();
                    options.write(true).create_new(true);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::OpenOptionsExt;
                        options.mode(0o600);
                    }
                    match options.open(path) {
                        Ok(mut file) => {
                            file.write_all(DEFAULT_CONFIG.as_bytes())?;
                            DEFAULT_CONFIG.into()
                        }
                        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => read_config(path)?,
                        Err(e) => return Err(e),
                    }
                }
                Err(e) => return Err(e),
            };
            Self::parse(&text)?.available_profiles()
        };
        load().map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn target(port: Option<&str>) -> Target {
        Target {
            host: "alice@host".into(),
            port: port.map(str::to_owned),
        }
    }
    fn run(profile: &Profile, target: &Target) -> std::process::Output {
        let (program, args) = profile.command(target).unwrap();
        Command::new(program).args(args).output().unwrap()
    }
    #[test]
    fn background_commands_capture_both_streams_and_close_stdin() {
        let output = background::run(
            "if read -r line; then exit 99; fi; printf output; printf diagnostic >&2; exit 7",
            true,
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stdout, b"output");
        assert_eq!(output.stderr, b"diagnostic");
        let output =
            background::run("printf discarded; exit 0", false, Duration::from_secs(1)).unwrap();
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn background_commands_have_deadlines_and_bounded_output() {
        for capture in [false, true] {
            let started = std::time::Instant::now();
            let error =
                background::run("sleep 30", capture, Duration::from_millis(50)).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            assert!(started.elapsed() < Duration::from_secs(2));
        }
        for (command, stream) in [
            ("while :; do printf '%01024d' 0; done", "stdout"),
            ("while :; do printf '%01024d' 0; done >&2", "stderr"),
        ] {
            let error = background::run(command, true, Duration::from_secs(1)).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert!(
                error
                    .to_string()
                    .contains(&format!("{stream} exceeds 64 KiB"))
            );
        }
        let output = background::run(
            "i=0; while [ \"$i\" -lt 64 ]; do printf '%01024d' 0; i=$((i + 1)); done",
            true,
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 65536);
    }

    #[test]
    #[cfg(unix)]
    fn background_descendants_cannot_keep_capture_open() {
        let started = std::time::Instant::now();
        let output = background::run(
            "sleep 30 & printf '%s' \"$!\"",
            true,
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(output.status.success());
        assert!(started.elapsed() < Duration::from_secs(1));
        let pid: u32 = String::from_utf8(output.stdout).unwrap().parse().unwrap();
        #[cfg(target_os = "linux")]
        {
            // An orphan can briefly remain as a zombie until init reaps it.
            // Neither a running descendant nor a retained pipe is acceptable.
            let deadline = std::time::Instant::now() + Duration::from_secs(1);
            loop {
                match fs::read_to_string(format!("/proc/{pid}/stat")) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => break,
                    Ok(stat)
                        if stat
                            .rsplit_once(") ")
                            .is_some_and(|(_, state)| state.starts_with('Z')) =>
                    {
                        break;
                    }
                    _ if std::time::Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    other => panic!("background descendant {pid} survived cleanup: {other:?}"),
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = pid;
    }

    #[test]
    fn background_descendants_stop_after_shell_exit() {
        let marker = std::env::temp_dir().join(format!("shui-descendant-{}", std::process::id()));
        let command = format!(
            "(sleep 0.3; printf escaped > {}) & printf done",
            shell_words::quote(&marker.to_string_lossy().replace('\\', "/")),
        );
        let output = background::run(&command, true, Duration::from_secs(2)).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"done");
        std::thread::sleep(Duration::from_millis(500));
        assert!(!marker.exists(), "descendant survived shell exit");
    }

    #[test]
    fn unsafe_shell_contexts_reject_template_substitutions() {
        for command in [
            "cat <<EOF\n%{input:x}\nEOF",
            "printf '%s' $((%{input:x}))",
            "printf '%s' $[%{input:x}]",
            "printf '%s' `printf '%s' %{input:x}`",
            "printf '%s\\n' \"$(printf '%s' \"%{input:x}\")\"",
            "printf '%s\\n' \"${SHUI_TEST_UNSET:-\"%{input:x}\"}\"",
            "printf '%s\\n' ${SHUI_TEST_UNSET:-%{input:x}}",
        ] {
            let profile = Profile {
                name: "Test".into(),
                key: None,
                available_command: None,
                command: command.into(),
                shell: Shell::Sh,
                port_args: vec![],
                prepare_command: None,
                prepare_shell: None,
                inputs: vec![],
                fields: HashMap::new(),
            };
            let values = HashMap::from([("x".into(), "$(printf INJECTED)".into())]);
            assert!(
                profile.command_with_inputs(&target(None), &values).is_err(),
                "{command}"
            );
        }
        let config = Config::parse("default: Safe\nprofiles:\n  - name: Safe\n    command: sh -c %{script} -- %{input:x}\n    script: |\n      cat <<EOF\n      $1\n      EOF\n    inputs: [{id: x, type: promptString, default: '$(printf INJECTED)'}]\n").unwrap();
        assert_eq!(
            run(&config.profiles[0], &target(None)).stdout,
            b"$(printf INJECTED)\n"
        );
    }

    #[test]
    fn oversized_runtime_expansion_returns_error_instead_of_panicking() {
        let config =
            Config::parse("default: SSH\nprofiles: [{name: SSH, command: 'ssh %h'}]").unwrap();
        let oversized = Target {
            host: "h".repeat(65537),
            port: None,
        };
        assert!(config.profiles[0].command(&oversized).is_err());
        assert!(config.profiles[0].history_line(&oversized).is_err());
        assert_eq!(config.history_profiles(&[oversized], "", "me"), [0]);
    }

    #[test]
    fn old_project_history_hints_remain_readable() {
        let config = Config::parse(DEFAULT_CONFIG).unwrap();
        let target = target(Some("2222"));
        let line = config.profiles[2].history_line(&target).unwrap();
        let old_line = line.replace(HISTORY_TARGET, LEGACY_HISTORY_TARGET);
        assert_eq!(history_target_hint(&old_line), Some("alice@host:2222"));
        assert_eq!(history_words(&line), history_words(&old_line));
        assert_eq!(config.history_profiles(&[target], &old_line, "me"), [2]);
    }

    #[test]
    fn username_display_defaults_and_validates_modes() {
        let yaml = "default: SSH\nprofiles: [{name: SSH, command: 'ssh %h'}]\n";
        assert_eq!(
            Config::parse(yaml).unwrap().username_display,
            UsernameDisplay::Auto
        );
        for (name, expected) in [
            ("auto", UsernameDisplay::Auto),
            ("always", UsernameDisplay::Always),
            ("never", UsernameDisplay::Never),
        ] {
            assert_eq!(
                Config::parse(&format!("{yaml}username_display: {name}\n"))
                    .unwrap()
                    .username_display,
                expected
            );
        }
        assert!(Config::parse(&format!("{yaml}username_display: invalid\n")).is_err());
    }
    #[test]
    fn minimal_profiles_and_literal_shell_substitution() {
        let config = Config::parse("default: Test\nprofiles:\n  - name: Test\n    command: printf '%s\\n' /sh://%h %{value}\n    value: \"a b ' \\\" $(printf INJECTED) ; $HOME %h\"\n").unwrap();
        let out = run(&config.profiles[0], &target(None));
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            "/sh://alice@host\na b ' \" $(printf INJECTED) ; $HOME %h\n"
        );
        Config::parse("default: SSH\nprofiles: [{name: SSH, command: 'ssh %h'}, {name: MC, command: 'mc /sh://%h'}]").unwrap();
    }
    #[test]
    fn nested_fields_quote_each_shell_layer_and_preserve_script() {
        let config = Config::parse("default: Test\nprofiles:\n  - name: Test\n    command: sh -c %{remote}\n    remote: sh -c %{script}\n    script: |\n      printf '%s\\n' \"spaces ' quotes\"\n      printf '%s\\n' '${literal%% *}'\n").unwrap();
        let out = run(&config.profiles[0], &target(None));
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            "spaces ' quotes\n${literal%% *}\n"
        );
        let line = config.profiles[0].history_line(&target(None)).unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(config.history_profiles(&[target(None)], &line, "me"), [0]);
    }
    #[test]
    fn multiline_transport_preserves_braces_unicode_and_legacy_history() {
        let script = "printf '%s\\n' 'café {one,two}'\nprintf '%s\\n' '\"quotes\" and \\slashes'\n";
        let (program, args) = Profile::shell_command(script.into());
        let output = Command::new(&program).args(&args).output().unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "café {one,two}\n\"quotes\" and \\slashes\n"
        );
        let line = shell_words::join(std::iter::once(program).chain(args));
        assert!(!line.contains('\n'));
        assert_eq!(
            history_words(&line),
            Some(shell_words::split(script).unwrap())
        );
        let legacy = format!("{SCRIPT_PREFIX}'printf old\\n'{SCRIPT_SUFFIX}");
        assert_eq!(
            history_words(&shell_words::join(["sh", "-c", &legacy])),
            Some(vec!["printf".into(), "old".into()])
        );
    }
    #[test]
    fn optional_ports_use_ordinary_shell_syntax() {
        let config = Config::parse("default: Test\nprofiles:\n  - name: Test\n    command: port=%p; printf '%s\\n' ${port:+-p \"$port\"} -- %h\n").unwrap();
        assert_eq!(
            String::from_utf8(run(&config.profiles[0], &target(None)).stdout).unwrap(),
            "--\nalice@host\n"
        );
        assert_eq!(
            String::from_utf8(run(&config.profiles[0], &target(Some("2222"))).stdout).unwrap(),
            "-p\n2222\n--\nalice@host\n"
        );
    }
    #[test]
    fn default_profiles_history_round_trip() {
        let windows = crate::config_defaults::defaults(true, false).unwrap();
        for source in [DEFAULT_CONFIG, windows.as_str()] {
            let config = Config::parse(source).unwrap();
            for (index, profile) in config.profiles.iter().enumerate() {
                let target = target(Some("2222"));
                let line = profile.history_line(&target).unwrap();
                assert!(!line.contains('\n'));
                assert_eq!(
                    config.history_profiles(std::slice::from_ref(&target), &line, "me"),
                    [index]
                );
                assert_eq!(
                    crate::history_target(&line, "me").unwrap().label(),
                    target.label()
                );
            }
        }
    }
    #[test]
    fn alternate_clients_preserve_port_and_user_arguments() {
        let config =
            Config::parse(&crate::config_defaults::defaults(true, false).unwrap()).unwrap();
        for (name, prefix, port_flag, separator) in [
            ("Dropbear", "", "-p", "--\n"),
            ("PuTTY", "-ssh\n", "-P", ""),
            ("Plink", "-ssh\n-t\n", "-P", ""),
        ] {
            let mut profile = config
                .profiles
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .clone();
            if profile.shell == Shell::Direct {
                for port in [None, Some("2222")] {
                    let (_, args) = profile.command(&target(port)).unwrap();
                    let mut expected = Vec::new();
                    if let Some(port) = port {
                        expected.extend([port_flag.to_owned(), port.to_owned()]);
                    }
                    expected.extend(prefix.lines().map(str::to_owned));
                    expected.extend(separator.lines().map(str::to_owned));
                    expected.push("alice@host".into());
                    assert_eq!(args, expected, "{name}");
                }
                continue;
            }
            // Execute the shipped shell template on every target, substituting
            // only the connection process with an argument recorder.
            profile.command = profile
                .command
                .replace(&format!("exec {}", profile.program()), "printf '%s\\n'");
            for port in [None, Some("2222")] {
                let output = run(&profile, &target(port));
                assert!(output.status.success(), "{name}: {:?}", output.stderr);
                let options = port.map_or(String::new(), |p| format!("{port_flag}\n{p}\n"));
                assert_eq!(
                    String::from_utf8(output.stdout).unwrap(),
                    format!("{prefix}{options}{separator}alice@host\n")
                );
            }
        }
    }
    #[test]
    fn alternate_clients_keep_users_that_only_openssh_would_supply() {
        let config =
            Config::parse(&crate::config_defaults::defaults(true, false).unwrap()).unwrap();
        let target = target(Some("2222"));
        let users = HashMap::from([("host".into(), "alice".into())]);
        for name in ["SSH", "Dropbear", "PuTTY", "Plink"] {
            let profile = config.profiles.iter().find(|p| p.name == name).unwrap();
            let normalized = profile.normalize_target(&target, &users);
            assert_eq!(
                normalized.host,
                if name == "SSH" { "host" } else { "alice@host" }
            );
            assert_eq!(normalized.port, target.port);
        }
    }
    #[test]
    fn history_matches_each_entry_and_ignores_unconfigured_commands() {
        let config = Config::parse(DEFAULT_CONFIG).unwrap();
        let targets = vec![target(None), target(Some("2222"))];
        assert_eq!(
            config.history_profiles(
                &targets,
                "mosh alice@host\nssh -p2222 alice@host exec bash -l\n",
                "me"
            ),
            [1, 3]
        );
        let line = config.profiles[2].history_line(&targets[1]).unwrap();
        assert_eq!(config.history_profiles(&targets, &line, "me"), [0, 2]);
        let line = format!("{line}\nexec sh -c 'unknown' # shui-target=alice@host:2222");
        assert_eq!(config.history_profiles(&targets, &line, "me"), [0, 0]);
    }
    #[test]
    fn invalid_templates_fail_without_executing() {
        for profile in [
            "{name: X, command: ''}",
            "{name: X, command: 'true', prepare_command: ''}",
            "{name: X, command: 'true', prepare_command: 'echo %{missing}'}",
            "{name: X, command: 'true', prepare_command: 'echo %{input:x}', inputs: [{id: x, type: promptString, default: test}]}",
            "{name: X, command: 'echo %{missing}'}",
            "{name: X, command: 'echo %{a}', a: '%{b}', b: '%{a}'}",
            "{name: X, command: 'echo %{unclosed'}",
            "{name: X, command: 'echo \"%h\"'}",
            "{name: X, command: 'echo ok', value: [not, a, string]}",
        ] {
            assert!(
                Config::parse(&format!("default: X\nprofiles: [{profile}]\n")).is_err(),
                "{profile}"
            );
        }
    }
    #[test]
    fn inputs_are_literal_across_nested_shell_layers() {
        let config =
            Config::parse(&crate::config_defaults::defaults(false, true).unwrap()).unwrap();
        let mut profile = config
            .configuration_scripts
            .iter()
            .find(|p| p.name == "Install tools")
            .unwrap()
            .clone();
        // Execute the documented quoting layers, replacing only transport and
        // the user-owned installation executable with an argument recorder.
        profile.command = "sh -c %{remote}".into();
        profile
            .fields
            .get_mut("script")
            .unwrap()
            .replace_range(.., "printf '%s\\n' --email \"$1\" --mode \"$2\"");
        let mut values = profile.input_defaults();
        assert!(profile.command_with_inputs(&target(None), &values).is_err());
        let literal = "spaces ' \" ; $(printf INJECTED) %{input:mode} %h";
        values.insert("email".into(), literal.into());
        values.insert("mode".into(), "full".into());
        let (program, args) = profile.command_with_inputs(&target(None), &values).unwrap();
        let output = Command::new(program).args(args).output().unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("--email\n{literal}\n--mode\nfull\n")
        );
        values.insert("mode".into(), "not-an-option".into());
        assert!(profile.command_with_inputs(&target(None), &values).is_err());
    }

    #[test]
    fn rejects_invalid_input_definitions_and_references() {
        for definition in [
            "{id: x, type: unsupported}",
            "{id: x, type: pickString, options: []}",
            "{id: x, type: pickString, options: [one], default: two}",
            "{id: x, type: promptString, options: [one]}",
            "{id: 'bad:id', type: promptString}",
            "{id: x, type: promptString, default: [invalid]}",
            "{id: x, type: promptString, pattern: '['}",
            "{id: x, type: promptString, pattern: '[0-9]+', default: abc}",
            "{id: x, type: pickString, pattern: '[0-9]+', options: ['1', abc]}",
            "{id: x, type: promptString, validation_message: ''}",
            "{id: x, type: promptString}, {id: x, type: promptString}",
        ] {
            assert!(Config::parse(&format!("default: SSH\nprofiles: [{{name: SSH, command: 'ssh %h'}}]\nconfiguration_scripts: [{{name: Setup, command: 'echo ok', inputs: [{definition}]}}]")).is_err(), "{definition}");
        }
        for command in [
            "echo %{input:missing}",
            "echo \"%{input:x}\"",
            "echo %{input:x",
        ] {
            let yaml = format!(
                "default: SSH\nprofiles: [{{name: SSH, command: 'ssh %h'}}]\nconfiguration_scripts:\n  - name: Setup\n    command: '{command}'\n    inputs: [{{id: x, type: promptString}}]\n"
            );
            assert!(Config::parse(&yaml).is_err(), "{command}");
        }
    }

    #[test]
    fn input_patterns_validate_complete_values_and_generated_defaults() {
        let input: Input = serde_yaml_ng::from_str(
            r#"
id: count
type: promptString
pattern: '[1-9][0-9]?'
validation_message: Enter a number from 1 to 99
"#,
        )
        .unwrap();
        input.validate().unwrap();
        for value in ["", "1", "99"] {
            input.validate_value(value).unwrap();
        }
        for value in ["0", "100", "x1", "1x", " 1", "1 ", "1\n"] {
            assert!(input.validate_value(value).is_err(), "{value:?}");
        }
        assert!(
            input
                .validate_value("x")
                .unwrap_err()
                .to_string()
                .contains("Enter a number from 1 to 99")
        );
        let mut required = input.clone();
        required.required = true;
        assert!(
            required
                .validate_value("")
                .unwrap_err()
                .to_string()
                .contains("is required")
        );
        required.command = Some("printf 'default: invalid'".into());
        required.validate().unwrap(); // Do not run generators at config load.
        assert!(required.resolve().is_err());
        required.command = Some("printf \"default: '42'\"".into());
        assert_eq!(required.resolve().unwrap().initial(), "42");
    }

    #[test]
    fn bundled_input_patterns_accept_formats_and_reject_malformed_values() {
        let config =
            Config::parse(&crate::config_defaults::defaults(false, false).unwrap()).unwrap();
        for (name, id, good, bad) in [
            (
                "SOCKS",
                "port",
                vec!["1", "1080", "65535"],
                vec!["0", "65536", "-1", "80/tcp"],
            ),
            (
                "Tunnel",
                "forward",
                vec!["8080:localhost:80", "1:[::1]:65535"],
                vec!["0:host:80", "8080:host:65536", "8080:host", "8080:$(id):80"],
            ),
            (
                "Configure log limits and cleanup",
                "disk",
                vec!["512M", "1", "2G"],
                vec!["0", "-1", "512MB", "1m", "1.5G"],
            ),
            (
                "Configure log limits and cleanup",
                "days",
                vec!["1", "36499", "36500"],
                vec!["0", "36501", "1.5", "-1"],
            ),
            (
                "Configure log limits and cleanup",
                "backups",
                vec!["1", "99", "100"],
                vec!["0", "101", "1.5"],
            ),
            (
                "Configure log limits and cleanup",
                "depth",
                vec!["0", "5"],
                vec!["6", "-1", "1.0"],
            ),
            (
                "Configure firewall",
                "ssh_ips",
                vec!["", "192.0.2.1,2001:db8::/64", "192.0.2.0/24 2001:db8::1"],
                vec!["not-an-ip", "192.0.2.1/abc", "$(id)"],
            ),
            (
                "Configure firewall",
                "ports",
                vec!["", "80/tcp 53/udp", "8000-8010/tcp,65535/udp"],
                vec![
                    "0/tcp",
                    "65536/udp",
                    "70000/tcp",
                    "80",
                    "80/sctp",
                    "80/tcp;",
                ],
            ),
        ] {
            let profile = config
                .profiles
                .iter()
                .chain(&config.configuration_scripts)
                .find(|p| p.name == name)
                .unwrap();
            let input = profile.inputs.iter().find(|i| i.id == id).unwrap();
            for value in good {
                input.validate_value(value).unwrap();
            }
            for value in bad {
                assert!(input.validate_value(value).is_err(), "{name}/{id}: {value}");
            }
        }
    }

    #[test]
    fn generated_input_configuration_is_validated() {
        let input: Input = serde_yaml_ng::from_str(
            r#"
id: generated
type: pickString
command: printf '%s' '{"options":["one","two"],"default":"two","description":"Generated"}'
"#,
        )
        .unwrap();
        input.validate().unwrap();
        let resolved = input.resolve().unwrap();
        assert_eq!(resolved.options, ["one", "two"]);
        assert_eq!(resolved.initial(), "two");
        assert_eq!(resolved.label(), "Generated");
        let mut prompt = input.clone();
        prompt.kind = InputType::PromptString;
        prompt.command = Some("printf 'default: generated text\\n'".into());
        assert_eq!(prompt.resolve().unwrap().initial(), "generated text");
        for command in [
            "exit 7",
            "printf 'not a mapping'",
            "printf 'options: []'",
            "printf 'options: [one]\\ndefault: two'",
            "printf 'type: promptString'",
        ] {
            let mut invalid = input.clone();
            invalid.command = Some(command.into());
            invalid.validate().unwrap(); // Loading definitions never runs commands.
            assert!(invalid.resolve().is_err(), "{command}");
        }
    }

    #[test]
    fn key_generator_lists_pairs_and_quotes_filenames() {
        let home = std::env::temp_dir().join(format!("shui-key-test-{}", std::process::id()));
        fs::create_dir_all(home.join(".ssh")).unwrap();
        for name in [
            "id_ed25519",
            "id_ed25519.pub",
            "id_work's key",
            "id_work's key.pub",
            "id_orphan.pub",
            "id_private_only",
            "other",
            "other.pub",
        ] {
            fs::write(home.join(".ssh").join(name), "fixture").unwrap();
        }
        let config =
            Config::parse(&crate::config_defaults::defaults(false, false).unwrap()).unwrap();
        assert_eq!(
            config.configuration_scripts[0].name,
            "Configure key-based auth"
        );
        assert_eq!(config.configuration_scripts[1].name, "Install dotfiles");
        assert!(
            config
                .configuration_scripts
                .iter()
                .take(2)
                .all(|p| p.available_command.is_none())
        );
        let mut input = config.configuration_scripts[0].inputs[0].clone();
        input.command = Some(format!(
            "HOME={}; export HOME; {}",
            shell_words::quote(&home.to_str().unwrap().replace('\\', "/")),
            input.command.unwrap()
        ));
        assert_eq!(
            input.resolve().unwrap().options,
            ["id_ed25519.pub", "id_work's key.pub"]
        );
        fs::remove_file(home.join(".ssh/id_ed25519")).unwrap();
        fs::remove_file(home.join(".ssh/id_work's key")).unwrap();
        assert_eq!(
            input.resolve().unwrap().options,
            ["Generate new Ed25519 key"]
        );
        assert!(!home.join(".ssh/id_ed25519").exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn rejects_conflicting_or_misplaced_profile_keys() {
        assert!(Config::parse("default: A\nprofiles: [{name: A, command: 'ssh %h', key: Alt+M}, {name: B, command: 'mosh %h', key: alt+m, available_command: 'exit 1'}]").is_err());
        assert!(Config::parse("default: A\nprofiles: [{name: A, command: 'ssh %h'}]\nconfiguration_scripts: [{name: Setup, command: 'true', key: Alt+M}]").is_err());
        for key in ["M", "Ctrl+C", "Alt+Enter", "Ctrl+Alt+H"] {
            assert!(
                Config::parse(&format!(
                    "default: A\nprofiles: [{{name: A, command: 'ssh %h', key: '{key}'}}]"
                ))
                .is_err()
            );
        }
    }

    #[test]
    fn profile_availability_preserves_order_and_available_default() {
        let config = Config::parse(
            "default: Preferred\nprofiles:\n  - {name: First, command: 'ssh %h'}\n  - {name: Missing, command: 'mc %h', available_command: 'exit 1'}\n  - {name: Preferred, command: 'mosh %h', available_command: 'exit 0'}\n",
        ).unwrap();
        assert_eq!(config.profiles.len(), 3, "parsing must not run checks");
        let available = config.available_profiles().unwrap();
        assert_eq!(
            available
                .profiles
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["First", "Preferred"]
        );
        assert_eq!(available.default_index(), 1);
        for command in ["''", "' '", "\"\\0\""] {
            assert!(Config::parse(&format!("default: X\nprofiles: [{{name: X, command: 'ssh %h', available_command: {command}}}]\n")).is_err());
        }
    }

    #[test]
    fn identical_availability_checks_run_once_per_load() {
        let path = std::env::temp_dir().join(format!("shui-check-cache-{}", std::process::id()));
        fs::write(&path, "").unwrap();
        let command = format!(
            "printf x >> {}",
            shell_words::quote(&path.to_str().unwrap().replace('\\', "/"))
        );
        let mut config = Config::parse(
            "default: A\nprofiles: [{name: A, command: 'ssh %h'}, {name: B, command: 'ssh %h'}]\n",
        )
        .unwrap();
        for profile in &mut config.profiles {
            profile.available_command = Some(command.clone());
        }
        let config = config.available_profiles().unwrap();
        assert_eq!(config.profiles.len(), 2);
        assert_eq!(fs::read_to_string(&path).unwrap(), "x");
        config.available_profiles().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "xx");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn config_path_uses_absolute_xdg_base_or_home_fallback() {
        let home = std::env::temp_dir().join("shui-home");
        let custom = home.join("custom-config");
        assert_eq!(
            config_path_with_base(&home, Some(&custom)),
            custom.join("shui.yml")
        );
        for base in [
            None,
            Some(Path::new("")),
            Some(Path::new("relative/config")),
        ] {
            assert_eq!(
                config_path_with_base(&home, base),
                home.join(".config/shui.yml")
            );
        }
    }

    #[test]
    fn creates_config_directory_and_preserves_existing_config() {
        let home = std::env::temp_dir().join(format!("shui-template-test-{}", std::process::id()));
        fs::create_dir_all(&home).unwrap();
        let path = config_path_with_base(&home, None);
        // Creating the defaults must work even on a client-free machine (Wine).
        // Availability may then reject opening the menu, without losing the file.
        if let Err(error) = Config::load_path(&path) {
            assert!(
                error
                    .to_string()
                    .contains("no launch profiles are available"),
                "{error}"
            );
        }
        assert!(!home.join(".ssh").exists());
        assert!(!home.join(".config/shui").exists());
        assert_eq!(fs::read_to_string(&path).unwrap(), DEFAULT_CONFIG);
        let custom = "default: Custom\nprofiles: [{name: Custom, command: 'ssh %h'}]\n";
        fs::write(&path, custom).unwrap();
        assert_eq!(Config::load_path(&path).unwrap().default, "Custom");
        assert_eq!(fs::read_to_string(&path).unwrap(), custom);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn configuration_reads_are_bounded_and_require_regular_files() {
        let directory =
            std::env::temp_dir().join(format!("shui-config-safety-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("config.yml");
        let oversized = "#".repeat(MAX_CONFIG_SIZE + 1);
        fs::write(&path, &oversized).unwrap();
        assert!(
            read_config(&path)
                .unwrap_err()
                .to_string()
                .contains("exceeds 1 MiB")
        );
        assert!(
            Config::parse(&oversized)
                .unwrap_err()
                .to_string()
                .contains("exceeds 1 MiB")
        );
        assert!(
            read_config(&directory)
                .unwrap_err()
                .to_string()
                .contains("regular file")
        );
        #[cfg(unix)]
        {
            let fifo = directory.join("fifo");
            nix::unistd::mkfifo(
                &fifo,
                nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
            )
            .unwrap();
            let started = std::time::Instant::now();
            assert!(
                read_config(&fifo)
                    .unwrap_err()
                    .to_string()
                    .contains("regular file")
            );
            assert!(started.elapsed() < Duration::from_secs(1));
        }
        fs::remove_dir_all(directory).unwrap();
    }
}

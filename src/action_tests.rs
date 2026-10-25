//! Shared action contracts: run the production event loops, on every target.
//! Only terminal I/O and external editor execution are replaced. No dispatcher
//! or platform-specific copy of the scenarios lives in this harness.
use crate::*;
use crossterm::event::KeyEvent;
use ratatui::{Frame, Terminal, backend::TestBackend, layout::Size};
use std::collections::VecDeque;

const CONFIG: &str = "
default: SSH
profiles:
  - {name: SSH, key: Alt+S, command: 'ssh %h'}
  - {name: Mosh, key: Ctrl+Alt+O, command: 'mosh %h'}
  - name: Input
    key: Alt+P
    command: 'ssh %h %{input:text} %{input:choice}'
    inputs:
      - {id: text, type: promptString, default: 'abc def'}
      - {id: choice, type: pickString, options: [one, two]}
configuration_scripts:
  - {name: First, command: 'echo first %h'}
  - {name: Second, command: 'echo second %h'}
";

struct Session {
    terminal: Terminal<TestBackend>,
    events: VecDeque<Event>,
    frames: Vec<String>,
    buffers: Vec<ratatui::buffer::Buffer>,
    edits: usize,
    fail_edits: usize,
}
impl Session {
    fn new(keys: impl IntoIterator<Item = KeyEvent>) -> Self {
        Self {
            terminal: Terminal::new(TestBackend::new(160, 24)).unwrap(),
            events: keys.into_iter().map(Event::Key).collect(),
            frames: Vec::new(),
            buffers: Vec::new(),
            edits: 0,
            fail_edits: 0,
        }
    }
    fn done(&self) {
        assert!(
            self.events.is_empty(),
            "action returned before consuming its scenario"
        );
    }
    fn saw(&self, text: &str) {
        assert!(
            self.frames.iter().any(|f| f.contains(text)),
            "missing {text:?} in {:?}",
            self.frames
        );
    }

    fn menu(&mut self, config: &mut launch::Config, direct: bool) -> Option<Selection> {
        let mut targets = if direct { vec![] } else { vec![host()] };
        let mut state = MenuState {
            profiles: vec![config.default_index(); targets.len()],
            query: if direct {
                host().label()
            } else {
                String::new()
            },
            ..MenuState::default()
        };
        let result = menu(self, &mut targets, config, &mut state, &missing_home()).unwrap();
        self.done();
        result
    }
}

fn missing_home() -> std::path::PathBuf {
    let home = env::temp_dir().join(format!("shui-action-missing-home-{}", std::process::id()));
    assert!(!home.exists());
    home
}
impl ui::Terminal for Session {
    fn draw(&mut self, draw: impl FnOnce(&mut Frame)) -> io::Result<()> {
        self.terminal.draw(draw).unwrap();
        self.buffers.push(self.terminal.backend().buffer().clone());
        self.frames.push(
            self.terminal
                .backend()
                .buffer()
                .content
                .chunks(160)
                .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        Ok(())
    }
    fn size(&self) -> io::Result<Size> {
        Ok(Size::new(160, 24))
    }
    fn read(&mut self) -> io::Result<Event> {
        Ok(self
            .events
            .pop_front()
            .expect("action did not produce its expected outcome"))
    }
    fn edit_config(&mut self, _: &Path) -> io::Result<launch::Config> {
        self.edits += 1;
        if self.edits <= self.fail_edits {
            Err(io::Error::other("test editor failure"))
        } else {
            launch::Config::parse(
                &CONFIG
                    .replace("name: SSH", "name: Updated")
                    .replace("default: SSH", "default: Updated"),
            )
        }
    }
}

fn key(name: &str) -> KeyEvent {
    // Use the configuration parser rather than maintaining another key parser.
    let bindings: bindings::Bindings =
        serde_yaml_ng::from_str(&format!("menu: {{quit: ['{name}']}}")).unwrap();
    bindings
        .test_actions("menu")
        .into_iter()
        .find(|(a, _)| a == "quit")
        .unwrap()
        .1[0]
}
fn host() -> Target {
    target("action-test.invalid", "").unwrap()
}
fn configuration(scope: &str, action: &str, remap: bool) -> launch::Config {
    launch::Config::parse(&format!(
        "{CONFIG}\n{}",
        if remap {
            format!("keybindings: {{{scope}: {{{action}: [F12]}}}}")
        } else {
            String::new()
        }
    ))
    .unwrap()
}

#[test]
fn every_binding_and_alias_has_an_outcome_in_its_real_event_loop() {
    for scope in ["menu", "dialog", "help", "editor"] {
        for (action, keys) in bindings::Bindings::default().test_actions(scope) {
            for (event, remap) in keys
                .into_iter()
                .map(|k| (k, false))
                .chain([(key("F12"), true)])
            {
                eprintln!("{scope}.{action}: {event:?}, remapped={remap}");
                let mut config = configuration(scope, &action, remap);
                if !remap {
                    config.keybindings = launch::Config::parse(
                        &crate::config_defaults::defaults(false, true).unwrap(),
                    )
                    .unwrap()
                    .keybindings;
                }
                match scope {
                    "menu" => menu_action(&action, event, config),
                    "dialog" => dialog_action(&action, event, config),
                    "help" => help_action(&action, event, &config),
                    "editor" => editor_action(&action, event, config),
                    _ => unreachable!(),
                }
            }
        }
    }
}

fn reveal(action: &str) -> Option<&'static str> {
    match action {
        "reveal_alt" => Some("Alt"),
        "reveal_ctrl" => Some("Ctrl"),
        "reveal_ctrl_alt" => Some("Ctrl+Alt"),
        "reveal_shift" => Some("Shift"),
        _ => None,
    }
}
fn check_reveal(session: &Session, action: &str) {
    let label = reveal(action).unwrap();
    assert!(
        session.frames[1]
            .lines()
            .any(|line| line.trim_start().starts_with(label))
    );
}

fn menu_action(action: &str, event: KeyEvent, mut config: launch::Config) {
    let mut targets: Vec<_> = (0..50)
        .map(|i| target(&format!("host{i:02}.invalid"), "").unwrap())
        .collect();
    let mut state = MenuState {
        profiles: vec![0; 50],
        selection: 25,
        ..MenuState::default()
    };
    let mut keys = vec![event];
    match action {
        "quit" | "connect" | "connect_and_return" => {}
        "connection_settings" => keys.push(key("Enter")),
        "configure_remote" => keys.extend([key("Space"), key("Enter")]),
        "new_remote" => keys.extend(
            "new.invalid"
                .chars()
                .map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
                .chain([key("Enter")]),
        ),
        "help" => keys.extend([key("Esc"), key("Esc")]),
        _ => keys.push(key("Esc")),
    }
    if matches!(action, "erase_character" | "erase_word" | "clear_search") {
        state.query = "host25 xyz".into();
    }
    let mut session = Session::new(keys);
    // This nonexistent home deliberately avoids consulting the developer's SSH files.
    let home = missing_home();
    let result = menu(&mut session, &mut targets, &mut config, &mut state, &home).unwrap();
    session.done();
    match action {
        "quit" => assert!(result.is_none()),
        "connect" | "connect_and_return" | "connection_settings" => {
            let result = result.unwrap();
            assert_eq!(result.target.host, "host25.invalid");
            assert_eq!(result.profile, 0);
            assert_eq!(result.return_to_menu, action == "connect_and_return");
            assert_eq!(
                result.connection_command.is_some(),
                action == "connection_settings"
            );
        }
        "configure_remote" => {
            let result = result.unwrap();
            assert_eq!(result.target.host, "host25.invalid");
            assert!(result.configure_only);
            assert_eq!(result.setup, Some(0));
            assert_eq!(
                command_args(result.setup_command.unwrap()),
                ["echo", "first", "host25.invalid"]
            );
        }
        "new_remote" => {
            let result = result.unwrap();
            assert!(result.new);
            assert_eq!(result.target.host, "new.invalid");
        }
        "help" => session.saw(" HELP "),
        "edit_config" => {
            assert_eq!(session.edits, 1);
            assert_eq!(config.profiles[0].name, "Updated");
        }
        "refresh" => {
            assert!(targets.is_empty());
            session.saw("Refreshed 0 hosts");
        }
        "clear_search" => assert!(state.query.is_empty()),
        "home" => assert_eq!(state.selection, 0),
        "end" => assert_eq!(state.selection, 49),
        "up" => assert_eq!(state.selection, 24),
        "down" => assert_eq!(state.selection, 26),
        "page_up" => assert_eq!(state.selection, 3),
        "page_down" => assert_eq!(state.selection, 47),
        "left" => assert_eq!(state.profiles[25], 2),
        "right" | "toggle_transport" => assert_eq!(state.profiles[25], 1),
        "erase_character" => assert_eq!(state.query, "host25 xy"),
        "erase_word" => assert_eq!(state.query, "host25 "),
        a if reveal(a).is_some() => check_reveal(&session, a),
        _ => panic!("missing menu outcome: {action}"),
    }
}

fn dialog_action(action: &str, event: KeyEvent, config: launch::Config) {
    if action == "toggle_recipe" {
        let mut session = Session::new([event, key("Down"), key("Enter")]);
        let selection = new_remote::dialog(&mut session, &config, Some(&host()), None)
            .unwrap()
            .unwrap();
        session.done();
        assert_eq!(selection.setup, Some(0));
        assert!(selection.additional_setup.is_empty());
        return;
    }
    let mut keys = vec![event];
    match action {
        "cancel" | "connect" | "connect_and_return" => {}
        "help" => keys.extend([key("Esc"), key("Enter")]),
        "next_field" => keys.extend([key("Right"), key("Enter")]),
        "previous_field" => keys.extend([key("Left"), key("Enter")]),
        _ => keys.push(key("Enter")),
    }
    // Left/right operate on the choice field; other actions begin at the text field.
    if matches!(action, "left" | "right") {
        keys.insert(0, key("Tab"));
    }
    let mut session = Session::new(keys);
    let result = new_remote::connect(&mut session, &config, &host(), 2, false).unwrap();
    session.done();
    match action {
        "cancel" => assert!(result.is_none()),
        "connect" | "connect_and_return" => {
            let result = result.unwrap();
            assert_eq!(result.target, host());
            assert_eq!(result.return_to_menu, action == "connect_and_return");
            assert_eq!(
                command_args(result.connection_command.unwrap()),
                ["ssh", "action-test.invalid", "abc def", "one"]
            );
        }
        "next_field" | "left" | "right" => assert_eq!(
            command_args(result.unwrap().connection_command.unwrap())
                .last()
                .unwrap(),
            "two"
        ),
        "previous_field" => assert_eq!(result.unwrap().profile, 1),
        "erase_character" => assert_eq!(
            command_args(result.unwrap().connection_command.unwrap()),
            ["ssh", "action-test.invalid", "abc de", "one"]
        ),
        "erase_word" => assert_eq!(
            command_args(result.unwrap().connection_command.unwrap()),
            ["ssh", "action-test.invalid", "abc ", "one"]
        ),
        "help" => session.saw(" HELP "),
        a if reveal(a).is_some() => check_reveal(&session, a),
        _ => panic!("missing dialog outcome: {action}"),
    }
}

fn help_action(action: &str, event: KeyEvent, config: &launch::Config) {
    let mut keys = Vec::new();
    if matches!(action, "home" | "up" | "page_up") {
        keys.push(key("PgDn"));
    }
    keys.push(event);
    if !matches!(action, "help" | "close") {
        keys.push(key("Esc"));
    }
    let mut session = Session::new(keys);
    help::show(&mut session, help::Context::Hosts, config).unwrap();
    session.done();
    let last = session.frames.last().unwrap();
    match action {
        "help" | "close" => assert_eq!(session.frames.len(), 1),
        "next_topic" => assert!(last.contains("Profiles · 1–")),
        "previous_topic" => assert!(last.contains("Keys · 1–")),
        "down" => assert!(last.contains("Guide · 2–")),
        "up" => assert!(last.contains("Guide · 22–")),
        "page_down" => assert!(last.contains("Guide · 23–")),
        "page_up" | "home" => assert!(last.contains("Guide · 1–")),
        "end" => {
            let footer = last.lines().last().unwrap();
            let range = footer.split("Guide · ").nth(1).unwrap().trim();
            let (_, end) = range.split_once('–').unwrap();
            let (end, total) = end.split_once('/').unwrap();
            assert_eq!(end, total);
        }
        a if reveal(a).is_some() => check_reveal(&session, a),
        _ => panic!("missing help outcome: {action}"),
    }
}

fn editor_action(action: &str, event: KeyEvent, mut config: launch::Config) {
    let mut keys = vec![event];
    match action {
        "cancel" | "edit" => {}
        "help" => keys.extend([key("Esc"), key("Esc")]),
        _ => keys.push(key("Esc")),
    }
    let mut session = Session::new(keys);
    session.fail_edits = 1;
    let mut profiles = [1, 0];
    editor::edit(&mut session, Path::new("."), &mut config, &mut profiles).unwrap();
    session.done();
    session.saw("test editor failure");
    match action {
        "cancel" => {
            assert_eq!(session.edits, 1);
            assert_eq!(config.profiles[0].name, "SSH");
        }
        "edit" => {
            assert_eq!(session.edits, 2);
            assert_eq!(config.profiles[0].name, "Updated");
            assert_eq!(profiles, [1, 0]);
        }
        "help" => session.saw(" HELP "),
        a if reveal(a).is_some() => check_reveal(&session, a),
        _ => panic!("missing editor outcome: {action}"),
    }
}

fn command_args(command: (String, Vec<String>)) -> Vec<String> {
    assert_eq!(command.0, "sh");
    assert_eq!(command.1[0], "-c");
    shell_words::split(&command.1[1]).unwrap()
}

#[test]
fn shipped_and_custom_profile_shortcuts_select_the_right_command() {
    let examples = crate::config_defaults::defaults(true, true).unwrap();
    for source in [launch::DEFAULT_CONFIG, examples.as_str(), CONFIG] {
        let count = launch::Config::parse(source).unwrap().profiles.len();
        for index in 0..count {
            for direct in [false, true] {
                let mut config = launch::Config::parse(source).unwrap();
                let Some(shortcut) = &config.profiles[index].key else {
                    continue;
                };
                let press = key(&shortcut.to_string());
                let mut release = press;
                release.kind = KeyEventKind::Release;
                let mut repeat = press;
                repeat.kind = KeyEventKind::Repeat;
                let mut keys = vec![release, repeat, press];
                if !config.profiles[index].inputs.is_empty() {
                    keys.push(key("Enter"));
                }
                let mut session = Session::new(keys);
                let selection = session.menu(&mut config, direct).unwrap();
                assert_eq!(selection.target, host());
                assert_eq!(selection.profile, index);
                assert_eq!(selection.new, direct);
                assert!(!selection.return_to_menu);
                assert_eq!(
                    selection.connection_command.is_some(),
                    !config.profiles[index].inputs.is_empty()
                );
                // Verify command construction too, without executing connection tools.
                let command = selection
                    .connection_command
                    .unwrap_or_else(|| config.profiles[index].command(&selection.target).unwrap());
                assert!(
                    launch::history_words(&shell_words::join(
                        std::iter::once(command.0.as_str())
                            .chain(command.1.iter().map(String::as_str))
                    ))
                    .unwrap()
                    .iter()
                    .any(|arg| arg.contains("action-test.invalid")),
                    "{} command lost its destination",
                    config.profiles[index].name
                );
            }
        }
    }
}

#[test]
fn releases_never_dispatch_actions_in_any_screen() {
    for scope in ["menu", "dialog", "help", "editor"] {
        for (action, keys) in bindings::Bindings::default().test_actions(scope) {
            for mut event in keys {
                eprintln!("release {scope}.{action}: {event:?}");
                event.kind = KeyEventKind::Release;
                let mut config = launch::Config::parse(CONFIG).unwrap();
                let mut session = Session::new([event, key("Esc")]);
                match scope {
                    "menu" => assert!(session.menu(&mut config, false).is_none()),
                    "dialog" => {
                        assert!(
                            new_remote::connect(&mut session, &config, &host(), 2, false)
                                .unwrap()
                                .is_none()
                        )
                    }
                    "help" => help::show(&mut session, help::Context::Hosts, &config).unwrap(),
                    "editor" => {
                        session.fail_edits = 1;
                        editor::edit(&mut session, Path::new("."), &mut config, &mut [0]).unwrap();
                        assert_eq!(session.edits, 1);
                    }
                    _ => unreachable!(),
                }
                session.done();
                // Include styles: moving focus can leave the text unchanged.
                assert_eq!(session.buffers[0], session.buffers[1]);
            }
        }
    }
}

#[test]
fn repeated_navigation_works_and_repeated_launches_do_not() {
    let mut config = launch::Config::parse(CONFIG).unwrap();
    for name in [
        "Enter",
        "Alt+Enter",
        "Ctrl+Enter",
        "F4",
        "Shift+F4",
        "F9",
        "F1",
        "Alt+S",
        "Ctrl+Alt+O",
        "Alt+P",
    ] {
        let mut event = key(name);
        event.kind = KeyEventKind::Repeat;
        let mut session = Session::new([event, key("Esc")]);
        assert!(session.menu(&mut config, false).is_none(), "{name}");
        assert_eq!(session.edits, 0);
    }
    for (action, name) in [
        ("left", "Left"),
        ("right", "Right"),
        ("up", "Up"),
        ("down", "Down"),
        ("page_up", "PgUp"),
        ("page_down", "PgDn"),
        ("erase_word", "Ctrl+Delete"),
    ] {
        let mut event = key(name);
        event.kind = KeyEventKind::Repeat;
        menu_action(action, event, launch::Config::parse(CONFIG).unwrap());
    }
}

#[test]
fn dialog_modes_validation_and_locked_fields_have_observable_outcomes() {
    let config = launch::Config::parse(CONFIG).unwrap();
    // Existing-host setup requires a recipe; switching modes preserves each draft.
    let mut session = Session::new([key("Enter"), key("Space"), key("Enter")]);
    let selection = new_remote::dialog(&mut session, &config, Some(&host()), None)
        .unwrap()
        .unwrap();
    session.done();
    session.saw("Choose a setup recipe");
    assert!(selection.configure_only);
    assert_eq!(selection.setup, Some(0));
    for direction in ["Left", "Right"] {
        let mut session = Session::new([
            key("Shift+Tab"),
            key("Shift+Tab"),
            key(direction),
            key(direction),
            key("Tab"),
            key("Tab"),
            key("Space"),
            key("Enter"),
        ]);
        let selection = new_remote::dialog(&mut session, &config, Some(&host()), None)
            .unwrap()
            .unwrap();
        session.done();
        assert_eq!(selection.target, host());
        assert!(selection.configure_only);
    }
    // Empty remote is rejected and focus moves to it; typing then confirms.
    let mut keys = vec![key("Enter")];
    keys.extend(
        "new.invalid"
            .chars()
            .map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)),
    );
    keys.push(key("Alt+Enter"));
    let mut session = Session::new(keys);
    let selection = new_remote::dialog(&mut session, &config, None, None)
        .unwrap()
        .unwrap();
    session.done();
    session.saw("Enter a valid host");
    assert!(selection.new && selection.return_to_menu);
    assert_eq!(selection.target.host, "new.invalid");
    // With a locked remote, both focus directions skip it; attempts to edit
    // the selected profile cannot silently change the destination.
    for direction in ["Tab", "Shift+Tab", "Down", "Up"] {
        let mut session = Session::new([key(direction), key("Delete"), key("Enter")]);
        let selection = new_remote::connect(&mut session, &config, &host(), 0, true)
            .unwrap()
            .unwrap();
        session.done();
        assert_eq!(selection.target, host());
    }
}

#[test]
fn setup_queue_preserves_order_inputs_and_unchecking() {
    let config = launch::Config::parse(
        r#"
default: SSH
profiles: [{name: SSH, command: 'ssh %h'}]
configuration_scripts:
  - name: First
    command: 'first %{input:value}'
    inputs: [{id: value, type: promptString, required: true}]
  - name: Second
    command: 'second %h'
"#,
    )
    .unwrap();
    let mut session = Session::new([
        key("Down"),
        key("Space"), // queue second first
        key("Up"),
        key("Space"),
        key("Enter"), // first has a missing input
        key("x"),
        key("Space"),
        key("y"),
        key("Shift+Tab"),
        key("Enter"),
    ]);
    let selection = new_remote::dialog(&mut session, &config, Some(&host()), None)
        .unwrap()
        .unwrap();
    session.done();
    session.saw("value is required");
    assert_eq!(selection.setup, Some(0));
    assert_eq!(
        command_args(selection.setup_command.unwrap()),
        ["first", "x y"]
    );
    assert_eq!(selection.additional_setup.len(), 1);
    assert_eq!(selection.additional_setup[0].0, 1);

    let mut session = Session::new([key("Space"), key("Space"), key("Enter"), key("Esc")]);
    assert!(
        new_remote::dialog(&mut session, &config, Some(&host()), None)
            .unwrap()
            .is_none()
    );
    session.done();
    session.saw("Choose a setup recipe");
}

#[test]
fn checkbox_inputs_toggle_independently_and_preserve_recipe_drafts() {
    let config = launch::Config::parse(
        r#"
default: Options
profiles:
  - name: Options
    command: 'launch %{input:first} %{input:second}'
    inputs: &options
      - {id: first, type: checkbox, description: First option, default: true}
      - {id: second, type: checkbox, description: Second option, default: false}
configuration_scripts:
  - name: Setup
    command: 'setup %{input:first} %{input:second}'
    inputs: *options
"#,
    )
    .unwrap();
    let mut session = Session::new(
        [
            "Space",
            "Down",
            "Space",
            "Down",
            "Space",
            "Up",
            "Space",
            "Shift+Tab",
            "Space",
            "Space",
            "Enter",
        ]
        .map(key),
    );
    let selection = new_remote::dialog(&mut session, &config, Some(&host()), None)
        .unwrap()
        .unwrap();
    session.done();
    session.saw("Toggle");
    assert_eq!(
        command_args(selection.setup_command.unwrap()),
        ["setup", "true", "true"]
    );
    let mut session = Session::new(["Space", "Down", "Space", "x", "Enter"].map(key));
    let selection = new_remote::connect(&mut session, &config, &host(), 0, false)
        .unwrap()
        .unwrap();
    session.done();
    assert_eq!(
        command_args(selection.connection_command.unwrap()),
        ["launch", "false", "true"]
    );
}

#[test]
fn pattern_errors_keep_dialogs_open_and_focus_the_invalid_input() {
    let config = launch::Config::parse(r#"
default: Input
profiles:
  - name: Input
    command: 'ssh %h %{input:count}'
    inputs: [{id: count, type: promptString, pattern: '[1-9]', validation_message: Use a digit from 1 to 9}]
configuration_scripts:
  - {name: First, command: 'first %h'}
  - name: Second
    command: 'second %{input:count}'
    inputs: [{id: count, type: promptString, pattern: '[1-9]', validation_message: Use a digit from 1 to 9}]
"#).unwrap();
    let mut session = Session::new(
        [
            "Space",
            "Down",
            "Space",
            "Down",
            "x",
            "Enter",
            "Backspace",
            "4",
            "Enter",
        ]
        .map(key),
    );
    let selection = new_remote::dialog(&mut session, &config, Some(&host()), None)
        .unwrap()
        .unwrap();
    session.done();
    session.saw("Use a digit from 1 to 9");
    assert_eq!(selection.setup, Some(0));
    assert_eq!(
        command_args(selection.additional_setup[0].1.clone()),
        ["second", "4"]
    );

    let mut session = Session::new(["x", "Enter", "Backspace", "5", "Enter"].map(key));
    let selection = new_remote::connect(&mut session, &config, &host(), 0, false)
        .unwrap()
        .unwrap();
    session.done();
    session.saw("Use a digit from 1 to 9");
    assert_eq!(
        command_args(selection.connection_command.unwrap()),
        ["ssh", "action-test.invalid", "5"]
    );
}

#[test]
fn required_inputs_and_literal_text_survive_the_full_dialog() {
    let config =
        launch::Config::parse(&CONFIG.replace("default: 'abc def'", "required: true")).unwrap();
    let text = "ü 'quoted' ; $(touch nope)";
    let mut keys = vec![key("Enter")];
    keys.extend(
        text.chars()
            .map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)),
    );
    keys.push(key("Alt+Enter"));
    let mut session = Session::new(keys);
    let selection = new_remote::connect(&mut session, &config, &host(), 2, false)
        .unwrap()
        .unwrap();
    session.done();
    session.saw("text is required");
    assert!(selection.return_to_menu);
    assert_eq!(
        command_args(selection.connection_command.unwrap()),
        ["ssh", "action-test.invalid", text, "one"]
    );
}

#[test]
fn legacy_control_bytes_and_disabled_bindings_reach_the_expected_outcome() {
    for letter in ['j', 'm'] {
        menu_action(
            "connect",
            KeyEvent::new(KeyCode::Char(letter), KeyModifiers::CONTROL),
            launch::Config::parse(CONFIG).unwrap(),
        );
    }
    dialog_action(
        "next_field",
        KeyEvent::new(KeyCode::Char('i'), KeyModifiers::CONTROL),
        launch::Config::parse(CONFIG).unwrap(),
    );
    for scope in ["menu", "dialog"] {
        let event = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL);
        let config = launch::Config::parse(CONFIG).unwrap();
        if scope == "menu" {
            menu_action("erase_word", event, config);
        } else {
            dialog_action("erase_word", event, config);
        }
    }
    let mut config = launch::Config::parse(&format!("{CONFIG}\nkeybindings: {{menu: {{connect: [], connect_and_return: [], connection_settings: [], edit_config: [], configure_remote: [], new_remote: []}}}}")).unwrap();
    let mut session = Session::new(
        [
            "Enter",
            "Alt+Enter",
            "Ctrl+Enter",
            "F9",
            "F4",
            "Shift+F4",
            "Esc",
        ]
        .map(key),
    );
    assert!(session.menu(&mut config, false).is_none());
    assert_eq!(session.edits, 0);
}

#[test]
fn direct_targets_profile_cycles_and_return_after_input_preserve_intent() {
    for direct in [false, true] {
        for (keys, expected_profile) in [
            (vec!["Right", "Enter"], 1),
            (vec!["Left", "Right", "Enter"], 0),
            (vec!["F8", "Enter"], 1),
            (vec!["F8", "F8", "Enter"], 0),
            (vec!["Left", "Alt+Enter", "Enter"], 2),
        ] {
            let returns = keys.contains(&"Alt+Enter");
            let mut config = launch::Config::parse(CONFIG).unwrap();
            let mut session = Session::new(keys.into_iter().map(key));
            let result = session.menu(&mut config, direct).unwrap();
            assert_eq!(result.target, host());
            assert_eq!(result.profile, expected_profile);
            assert_eq!(result.new, direct);
            assert_eq!(result.return_to_menu, returns);
        }
    }
    for query in ["", "-unsafe", "user@host:65536"] {
        let mut config = launch::Config::parse(CONFIG).unwrap();
        let mut state = MenuState {
            query: query.into(),
            ..MenuState::default()
        };
        let mut session = Session::new(
            [
                "Enter",
                "Alt+Enter",
                "Ctrl+Enter",
                "Alt+S",
                "Left",
                "Right",
                "F8",
                "Esc",
            ]
            .map(key),
        );
        assert!(
            menu(
                &mut session,
                &mut vec![],
                &mut config,
                &mut state,
                &missing_home()
            )
            .unwrap()
            .is_none()
        );
        session.done();
        assert_eq!(state.query, query);
        assert_eq!(state.direct_profile, None);
    }
}

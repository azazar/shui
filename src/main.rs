#[cfg(test)]
mod action_tests;
mod bar;
mod bindings;
mod cli;
mod command;
#[cfg(test)]
mod config_defaults;
#[cfg(all(test, unix))]
mod docker_tests;
mod editor;
mod help;
mod launch;
mod new_remote;
#[cfg(all(test, unix))]
mod preset_tests;
mod search;
mod shortcut;
mod ssh_config;
mod ui;
#[cfg(all(test, windows))]
mod windows_tests;

use crossterm::{
    cursor::Show,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    style::{Attribute, ResetColor, SetAttribute},
};
use ratatui::{
    DefaultTerminal,
    layout::{Alignment, Constraint, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Cell, List, ListItem, ListState, Paragraph, Row, Table, TableState},
};
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    io::{self, Write},
    path::Path,
    process::Command,
};

// Catppuccin Mocha: https://catppuccin.com/palette/
mod theme {
    use ratatui::style::Color;
    pub const BASE: Color = Color::Rgb(30, 30, 46);
    pub const TEXT: Color = Color::Rgb(205, 214, 244);
    pub const SUBTEXT: Color = Color::Rgb(166, 173, 200);
    pub const SURFACE: Color = Color::Rgb(49, 50, 68);
    pub const SELECTION: Color = Color::Rgb(69, 71, 90);
    pub const LAVENDER: Color = Color::Rgb(180, 190, 254);
    pub const USERNAME: Color = Color::Rgb(249, 226, 175);
    pub const PEACH: Color = Color::Rgb(250, 179, 135);
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    host: String,
    port: Option<String>,
}
impl Target {
    fn label(&self) -> String {
        match &self.port {
            Some(p) => format!("{}:{p}", self.host),
            None => self.host.clone(),
        }
    }
}

fn display_usernames<'a>(
    targets: &'a [Target],
    users: &'a HashMap<String, String>,
    mode: launch::UsernameDisplay,
) -> Vec<&'a str> {
    let parts: Vec<_> = targets
        .iter()
        .map(|target| {
            let (explicit, host) = target
                .host
                .rsplit_once('@')
                .map_or((None, target.host.as_str()), |(user, host)| {
                    (Some(user), host)
                });
            let user = explicit
                .or_else(|| users.get(host).map(String::as_str))
                .unwrap_or("");
            (user, host, target.port.as_deref())
        })
        .collect();
    let mut accounts: HashMap<_, HashSet<_>> = HashMap::new();
    for (user, host, port) in &parts {
        accounts.entry((*host, *port)).or_default().insert(*user);
    }
    parts
        .into_iter()
        .map(|(user, host, port)| match mode {
            launch::UsernameDisplay::Always => user,
            launch::UsernameDisplay::Never => "",
            launch::UsernameDisplay::Auto if accounts[&(host, port)].len() > 1 => user,
            launch::UsernameDisplay::Auto => "",
        })
        .collect()
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('-')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.-@[]:".contains(c))
}
fn target(raw: &str, _user: &str) -> Option<Target> {
    let (host, port) = match raw.rsplit_once(':') {
        Some((h, p))
            if !p.is_empty()
                && p.chars().all(|c| c.is_ascii_digit())
                && (!h.contains(':') || h.ends_with(']')) =>
        {
            (h, Some(p.to_owned()))
        }
        _ => (raw, None),
    };
    if host.len() > 1024
        || !valid_host(host)
        || port
            .as_ref()
            .is_some_and(|value| value.parse::<u16>().ok().filter(|port| *port > 0).is_none())
    {
        return None;
    }
    Some(Target {
        host: host.into(),
        port,
    })
}
fn history_target(line: &str, user: &str) -> Option<Target> {
    if let Some(hint) = launch::history_target_hint(line) {
        return target(hint, user);
    }
    let words = launch::history_words(line)?;
    if let [program, path] = words.as_slice()
        && program == "mc"
        && let Some(host) = path
            .strip_prefix("/sh://")
            .and_then(|p| p.strip_suffix('/'))
    {
        return target(host, user);
    }
    history_connection(line, user).map(|(target, _, _)| target)
}

fn history_connection(line: &str, user: &str) -> Option<(Target, Vec<String>, String)> {
    let words = launch::history_words(line)?;
    let mut it = words.iter().map(String::as_str);
    let cmd = it.next()?;
    if !matches!(cmd, "ssh" | "ssh-copy-id" | "mosh") {
        return None;
    }
    let mut port = None;
    let mut login = None;
    let host = loop {
        let word = it.next()?;
        if word == "--" {
            break it.next()?;
        }
        if word == "-p" {
            port = Some(it.next()?.to_owned());
            continue;
        }
        if word.starts_with("-p") && word.len() > 2 {
            port = Some(word[2..].to_owned());
            continue;
        }
        if word == "-l" {
            login = Some(it.next()?);
            continue;
        }
        if word == "--ssh" || word.starts_with("--ssh=") {
            let value = word.strip_prefix("--ssh=").or_else(|| it.next())?;
            port = history_target(&format!("{value} placeholder"), user).and_then(|t| t.port);
            continue;
        }
        if matches!(
            word,
            "-i" | "-F"
                | "-o"
                | "-J"
                | "-L"
                | "-R"
                | "-D"
                | "-S"
                | "-b"
                | "-c"
                | "-E"
                | "-e"
                | "-m"
                | "-O"
                | "-W"
                | "-w"
        ) {
            it.next()?;
            continue;
        }
        if word.starts_with('-') {
            continue;
        }
        break word;
    };
    let mut t = target(host, user)?;
    if let Some(p) = port {
        p.parse::<u16>().ok().filter(|p| *p > 0)?;
        t.port = Some(p);
    }
    if let Some(login) = login {
        t = target(&format!("{login}@{}", t.label()), user)?;
    }
    Some((t, it.map(str::to_owned).collect(), cmd.to_owned()))
}
fn discover(home: &Path, user: &str) -> Vec<Target> {
    let ssh = ssh_config::load(home, user);
    let mut entries = HashMap::new();
    let mut rank = 0usize;
    let mut add = |t: Target| {
        let t = ssh_config::normalize_target(&t, &ssh.users);
        rank += 1;
        entries.insert(t.label(), (rank, t));
    };
    for entry in ssh.targets {
        add(entry);
    }
    for line in read_history(&home.join(".config/mc/hotlist"))
        .unwrap_or_default()
        .lines()
    {
        if let Ok(words) = shell_words::split(line)
            && words.first().map(String::as_str) == Some("ENTRY")
            && let Some(url) = words.windows(2).find(|w| w[0] == "URL").map(|w| &w[1])
            && let Some(rest) = url.strip_prefix("/sh://")
            && let Some(t) = target(rest.split('/').next().unwrap_or(""), user)
        {
            add(t);
        }
    }
    for line in read_history(&home.join(".bash_history"))
        .unwrap_or_default()
        .lines()
    {
        if let Some(t) = history_target(line, user) {
            add(t);
        }
    }
    let mut values: Vec<_> = entries.into_values().collect();
    values.sort_by_key(|(rank, _)| std::cmp::Reverse(*rank));
    values.into_iter().map(|(_, t)| t).collect()
}
fn erase_filter_word(query: &mut String) {
    let trimmed = query.trim_end();
    let end = trimmed
        .rfind(char::is_whitespace)
        .map_or(0, |i| i + trimmed[i..].chars().next().unwrap().len_utf8());
    query.truncate(end);
}

// Keep the end of an edited field visible without splitting wide or combining text.
fn text_tail(text: &str, width: usize) -> String {
    let span = Span::raw(text);
    if span.width() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let graphemes: Vec<_> = span.styled_graphemes(Style::default()).collect();
    let mut used = 1; // Leading ellipsis.
    let mut start = graphemes.len();
    for grapheme in graphemes.iter().rev() {
        used += Span::raw(grapheme.symbol).width();
        if used > width {
            break;
        }
        start -= 1;
    }
    let mut result = String::from("…");
    for grapheme in &graphemes[start..] {
        result.push_str(grapheme.symbol);
    }
    result
}

struct Selection {
    target: Target,
    profile: usize,
    setup: Option<usize>,
    setup_command: Option<(String, Vec<String>)>,
    additional_setup: Vec<(usize, (String, Vec<String>))>,
    new: bool,
    configure_only: bool,
    return_to_menu: bool,
    connection_command: Option<(String, Vec<String>)>,
}

#[derive(Default)]
struct MenuState {
    query: String,
    selection: usize,
    profiles: Vec<usize>,
    notice: Option<String>,
    table: TableState,
    direct_profile: Option<usize>,
}

fn menu(
    terminal: &mut impl ui::Terminal,
    targets: &mut Vec<Target>,
    config: &mut launch::Config,
    state: &mut MenuState,
    home: &Path,
) -> io::Result<Option<Selection>> {
    let MenuState {
        query,
        selection,
        profiles,
        notice,
        table: table_state,
        direct_profile,
    } = state;
    let mut bar_state = bar::State::default();
    let local_user = local_username();
    let ssh = ssh_config::load(home, &local_user);
    let mut host_config = ssh.hints;
    let mut configured_users = ssh.users;
    loop {
        let footer_height = 1;
        let visible = search::matching(targets, query, &host_config);
        *selection = (*selection).min(visible.len().saturating_sub(1));
        let username_labels =
            display_usernames(targets, &configured_users, config.username_display);
        let profile_index = visible.get(*selection).map_or(
            direct_profile.unwrap_or_else(|| config.default_index()),
            |i| profiles[*i],
        );
        let direct_target = visible
            .is_empty()
            .then(|| new_remote::parse_remote(query))
            .flatten();
        let has_target = !visible.is_empty() || direct_target.is_some();
        terminal.draw(|frame| {
            frame.render_widget(
                Block::default().style(Style::default().fg(theme::TEXT).bg(theme::BASE)),
                frame.area(),
            );
            let areas = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(footer_height),
            ])
            .split(frame.area());
            let count = format!(" {}/{} ", visible.len(), targets.len());
            let query_width = usize::from(areas[0].width).saturating_sub(6 + count.len() + 3);
            let header = Line::from(vec![
                Span::styled(
                    " SHUI ",
                    Style::default()
                        .fg(theme::BASE)
                        .bg(theme::LAVENDER)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(count, Style::default().fg(theme::SUBTEXT)),
                Span::styled(
                    format!(" / {}", text_tail(query, query_width)),
                    Style::default().fg(theme::LAVENDER),
                ),
            ]);
            frame.render_widget(
                Paragraph::new(header).style(Style::default().bg(theme::SURFACE)),
                areas[0],
            );
            let rows: Vec<_> = visible
                .iter()
                .map(|i| {
                    let profile = &config.profiles[profiles[*i]];
                    let mosh = profile.program() == "mosh";
                    let label = targets[*i].label();
                    let host = label
                        .rsplit_once('@')
                        .map_or(label.as_str(), |(_, host)| host);
                    let username = username_labels[*i];
                    Row::new(vec![
                        Cell::from(profile.name.clone()).style(Style::default().fg(if mosh {
                            theme::PEACH
                        } else {
                            theme::SUBTEXT
                        })),
                        Cell::from(Line::from(username.to_owned()).alignment(Alignment::Right))
                            .style(
                                Style::default()
                                    .fg(theme::USERNAME)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        Cell::from(host.to_owned()),
                    ])
                })
                .collect();
            let mut hint_area = areas[1];
            if rows.is_empty() {
                frame.render_widget(
                    Paragraph::new(if let Some(target) = new_remote::parse_remote(query) {
                        format!(
                            " {} connects to {} · {}",
                            config.keybindings.label("menu", "connect"),
                            target.label(),
                            config.profiles[profile_index].name,
                        )
                    } else if targets.is_empty() {
                        format!(
                            " Type a host, then {} to connect.\n {} setup · {} help",
                            config.keybindings.label("menu", "connect"),
                            config.keybindings.label("menu", "new_remote"),
                            config.keybindings.label("menu", "help")
                        )
                    } else {
                        format!(
                            " No matches · {} clears the search",
                            config.keybindings.label("menu", "clear_search")
                        )
                    })
                    .wrap(ratatui::widgets::Wrap { trim: false })
                    .style(Style::default().fg(theme::SUBTEXT)),
                    areas[1],
                );
            } else {
                let profile_width = config
                    .profiles
                    .iter()
                    .map(|profile| Line::from(profile.name.as_str()).width())
                    .max()
                    .unwrap_or(0)
                    .min(usize::from(areas[1].width.saturating_sub(4) / 3))
                    as u16;
                let username_width = username_labels
                    .iter()
                    .map(|user| Line::from(*user).width())
                    .max()
                    .unwrap_or(0)
                    .min(usize::from(areas[1].width.saturating_sub(4) / 4))
                    as u16;
                let host_width = visible
                    .iter()
                    .map(|i| {
                        let label = targets[*i].label();
                        Span::raw(
                            label
                                .rsplit_once('@')
                                .map_or(label.as_str(), |(_, host)| host),
                        )
                        .width()
                    })
                    .max()
                    .unwrap_or(0);
                let row_width = usize::from(profile_width + username_width) + host_width + 4;
                let hint_width = usize::from((areas[1].width / 2).min(80));
                if row_width + hint_width > usize::from(areas[1].width) {
                    // A hint may use empty rows, but must never cover host text.
                    let occupied = visible.len().min(usize::from(areas[1].height)) as u16;
                    hint_area.y += occupied;
                    hint_area.height -= occupied;
                }
                let table = Table::new(
                    rows,
                    [
                        Constraint::Length(profile_width),
                        Constraint::Length(username_width),
                        Constraint::Min(1),
                    ],
                )
                .column_spacing(1)
                .highlight_symbol("› ")
                .row_highlight_style(
                    Style::default()
                        .bg(theme::SELECTION)
                        .add_modifier(Modifier::BOLD),
                );
                if visible.len() <= usize::from(areas[1].height) {
                    *table_state.offset_mut() = 0;
                }
                table_state.select(Some(*selection));
                frame.render_stateful_widget(table, areas[1], table_state);
            }
            let keys = &config.keybindings;
            let mut items = vec![
                bar::Item::profiles(config, profile_index).available(has_target),
                bar::Item::action(keys, "menu", "connect", "Open", 2).available(has_target),
                bar::Item::action(keys, "menu", "connect_and_return", "Return", 4)
                    .available(has_target),
                bar::Item::action(keys, "menu", "connection_settings", "Edit", 5)
                    .available(has_target),
                bar::Item::action(keys, "menu", "configure_remote", "Setup", 6),
                bar::Item::action(keys, "menu", "new_remote", "New", 8),
                bar::Item::action(keys, "menu", "edit_config", "Config", 7),
                bar::Item::action(keys, "menu", "refresh", "Refresh", 9),
                bar::Item::action(keys, "menu", "clear_search", "Clear", 10)
                    .available(!query.is_empty()),
                bar::Item::action(keys, "menu", "help", "Help", 0),
                bar::Item::action(keys, "menu", "quit", "Quit", 1),
            ];
            for (actions, available) in [
                (&["toggle_transport"][..], has_target),
                (
                    &["home", "end", "up", "down", "page_up", "page_down"][..],
                    !visible.is_empty(),
                ),
                (&["erase_character", "erase_word"][..], !query.is_empty()),
            ] {
                if !available {
                    items.extend(actions.iter().map(|action| {
                        bar::Item::action(keys, "menu", action, "", 10).available(false)
                    }));
                }
            }
            bar_state.draw(frame, areas[2], config, "menu", items);
            if let Some(i) = visible.get(*selection)
                && let Some(block) = host_config.get(&targets[*i].host)
            {
                search::hint(frame, hint_area, block);
            }
            if let Some(message) = notice.as_deref() {
                // Reuse the footer so results cannot cover or move the host list.
                let rect = areas[2];
                frame.render_widget(ratatui::widgets::Clear, rect);
                frame.render_widget(
                    Paragraph::new(message)
                        .style(Style::default().fg(theme::USERNAME).bg(theme::SURFACE)),
                    rect,
                );
            }
        })?;
        let event = terminal.read()?;
        if matches!(event, Event::FocusLost) {
            bar_state.reset();
        }
        if let Event::Key(key) = event {
            if bar_state.event(key, &config.keybindings, "menu") {
                notice.take();
                continue;
            }
            if key.kind == KeyEventKind::Release {
                continue;
            }
            notice.take();
            if key.kind == KeyEventKind::Press
                && let Some(profile) = config
                    .profiles
                    .iter()
                    .position(|p| p.key.as_ref().is_some_and(|shortcut| shortcut.matches(key)))
            {
                let target = visible
                    .get(*selection)
                    .map(|i| targets[*i].clone())
                    .or_else(|| direct_target.clone());
                if let Some(target) = target {
                    let target =
                        config.profiles[profile].normalize_target(&target, &configured_users);
                    if !config.profiles[profile].inputs.is_empty() {
                        bar_state.reset();
                        if let Some(mut result) =
                            new_remote::connect(terminal, config, &target, profile, false)?
                        {
                            result.new = visible.is_empty();
                            return Ok(Some(result));
                        }
                        continue;
                    }
                    return Ok(Some(Selection {
                        target,
                        profile,
                        setup: None,
                        setup_command: None,
                        additional_setup: Vec::new(),
                        new: visible.is_empty(),
                        configure_only: false,
                        return_to_menu: key.modifiers.contains(KeyModifiers::ALT)
                            && key.code == KeyCode::Enter,
                        connection_command: None,
                    }));
                }
                continue;
            }
            let repeated = key.kind == KeyEventKind::Repeat;
            let Some(key) = config.keybindings.translate("menu", key) else {
                continue;
            };
            if repeated
                && !matches!(
                    key.code,
                    KeyCode::Char(_)
                        | KeyCode::Up
                        | KeyCode::Down
                        | KeyCode::Left
                        | KeyCode::Right
                        | KeyCode::PageUp
                        | KeyCode::PageDown
                        | KeyCode::Backspace
                        | KeyCode::Delete
                )
            {
                continue;
            }
            match key.code {
                KeyCode::F(1) => {
                    bar_state.reset();
                    help::show(terminal, help::Context::Hosts, config)?;
                }
                KeyCode::F(5) => {
                    let selected = visible.get(*selection).map(|index| targets[*index].clone());
                    let choices: HashMap<_, _> = targets
                        .iter()
                        .zip(profiles.iter())
                        .map(|(target, profile)| (target.label(), *profile))
                        .collect();
                    let user = local_username();
                    *targets = discover(home, &user);
                    *profiles = config.history_profiles_with_users(
                        targets,
                        &read_history(&home.join(".bash_history")).unwrap_or_default(),
                        &user,
                        &ssh_config::load(home, &user).users,
                    );
                    for (index, target) in targets.iter().enumerate() {
                        if let Some(profile) = choices.get(&target.label()) {
                            profiles[index] = *profile;
                        }
                    }
                    let ssh = ssh_config::load(home, &user);
                    host_config = ssh.hints;
                    configured_users = ssh.users;
                    *selection = search::matching(targets, query, &host_config)
                        .iter()
                        .position(|index| selected.as_ref() == Some(&targets[*index]))
                        .unwrap_or(0);
                    *notice = Some(format!("Refreshed {} hosts", targets.len()));
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    query.clear();
                    *selection = 0;
                }
                KeyCode::Home => *selection = 0,
                KeyCode::End => *selection = visible.len().saturating_sub(1),
                KeyCode::F(9) => {
                    bar_state.reset();
                    editor::edit(terminal, home, config, profiles)?;
                    *direct_profile = None;
                }
                KeyCode::F(4) => {
                    bar_state.reset();
                    if let Some(mut selection) = new_remote::dialog(
                        terminal,
                        config,
                        if key.modifiers.contains(KeyModifiers::SHIFT) {
                            None
                        } else {
                            visible.get(*selection).map(|i| &targets[*i])
                        },
                        if key.modifiers.contains(KeyModifiers::SHIFT) {
                            None
                        } else {
                            direct_target.as_ref()
                        },
                    )? {
                        if !selection.configure_only
                            && !config.profiles[selection.profile].inputs.is_empty()
                        {
                            let Some(connection) = new_remote::connect(
                                terminal,
                                config,
                                &selection.target,
                                selection.profile,
                                selection.setup.is_some(),
                            )?
                            else {
                                continue;
                            };
                            selection.target = connection.target;
                            selection.profile = connection.profile;
                            selection.connection_command = connection.connection_command;
                            selection.return_to_menu |= connection.return_to_menu;
                        }
                        return Ok(Some(selection));
                    }
                }
                KeyCode::Esc => return Ok(None),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(None);
                }
                // Legacy terminals encode Ctrl-Backspace as Ctrl-H (0x08).
                KeyCode::Backspace | KeyCode::Delete | KeyCode::Char('h')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    erase_filter_word(query);
                    *selection = 0;
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    query.push(c);
                    *selection = 0;
                }
                KeyCode::Backspace | KeyCode::Delete => {
                    query.pop();
                    *selection = 0;
                }
                KeyCode::Up => *selection = selection.saturating_sub(1),
                KeyCode::Down => *selection = selection.saturating_add(1),
                KeyCode::PageUp => {
                    *selection = selection.saturating_sub(
                        terminal
                            .size()?
                            .height
                            .saturating_sub(footer_height + 1)
                            .max(1) as usize,
                    )
                }
                KeyCode::PageDown => {
                    *selection = selection.saturating_add(
                        terminal
                            .size()?
                            .height
                            .saturating_sub(footer_height + 1)
                            .max(1) as usize,
                    )
                }
                KeyCode::Left => {
                    if let Some(i) = visible.get(*selection) {
                        profiles[*i] =
                            (profiles[*i] + config.profiles.len() - 1) % config.profiles.len();
                    } else if direct_target.is_some() {
                        *direct_profile = Some(
                            (profile_index + config.profiles.len() - 1) % config.profiles.len(),
                        );
                    }
                }
                KeyCode::Right => {
                    if let Some(i) = visible.get(*selection) {
                        profiles[*i] = (profiles[*i] + 1) % config.profiles.len();
                    } else if direct_target.is_some() {
                        *direct_profile = Some((profile_index + 1) % config.profiles.len());
                    }
                }
                KeyCode::F(8) => {
                    if let Some(i) = visible.get(*selection) {
                        profiles[*i] = config.toggle_transport(profiles[*i]);
                    } else if direct_target.is_some() {
                        *direct_profile = Some(config.toggle_transport(profile_index));
                    }
                }
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    bar_state.reset();
                    let target = visible
                        .get(*selection)
                        .map(|index| targets[*index].clone())
                        .or_else(|| new_remote::parse_remote(query));
                    if let Some(target) = target
                        && let Some(mut result) =
                            new_remote::connect(terminal, config, &target, profile_index, false)?
                    {
                        result.new = visible.is_empty();
                        return Ok(Some(result));
                    }
                }
                KeyCode::Enter if visible.is_empty() => {
                    if let Some(target) = new_remote::parse_remote(query) {
                        let profile = profile_index;
                        let target =
                            config.profiles[profile].normalize_target(&target, &configured_users);
                        if !config.profiles[profile].inputs.is_empty() {
                            bar_state.reset();
                            if let Some(mut result) =
                                new_remote::connect(terminal, config, &target, profile, false)?
                            {
                                result.new = true;
                                result.return_to_menu |= key.modifiers.contains(KeyModifiers::ALT);
                                return Ok(Some(result));
                            }
                        } else {
                            return Ok(Some(Selection {
                                target,
                                profile,
                                setup: None,
                                setup_command: None,
                                additional_setup: Vec::new(),
                                new: true,
                                configure_only: false,
                                return_to_menu: key.modifiers.contains(KeyModifiers::ALT),
                                connection_command: None,
                            }));
                        }
                    }
                }
                KeyCode::Enter => {
                    if let Some(i) = visible.get(*selection) {
                        if !config.profiles[profile_index].inputs.is_empty() {
                            bar_state.reset();
                            if let Some(mut result) = new_remote::connect(
                                terminal,
                                config,
                                &targets[*i],
                                profile_index,
                                false,
                            )? {
                                result.return_to_menu |= key.modifiers.contains(KeyModifiers::ALT);
                                return Ok(Some(result));
                            }
                            continue;
                        }
                        return Ok(Some(Selection {
                            target: targets[*i].clone(),
                            profile: profile_index,
                            setup: None,
                            setup_command: None,
                            additional_setup: Vec::new(),
                            new: false,
                            configure_only: false,
                            return_to_menu: key.modifiers.contains(KeyModifiers::ALT)
                                && key.code == KeyCode::Enter,
                            connection_command: None,
                        }));
                    }
                }
                _ => {}
            }
        }
    }
}
fn init_terminal() -> io::Result<DefaultTerminal> {
    let terminal = ratatui::try_init()?;
    if let Err(err) = keyboard_enhancements(&mut io::stdout(), true)
        .and_then(|()| execute!(io::stdout(), event::EnableFocusChange))
    {
        let _ = suspend_terminal();
        return Err(err);
    }
    Ok(terminal)
}

fn keyboard_enhancements(output: &mut impl Write, enable: bool) -> io::Result<()> {
    // Crossterm reads native Windows key events, and its Kitty protocol
    // commands unconditionally return Unsupported on Windows.
    #[cfg(windows)]
    {
        let _ = (output, enable);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        use crossterm::event::{
            KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
        };
        if enable {
            execute!(
                output,
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                        // All-key reporting otherwise supplies unshifted key codes,
                        // losing text such as uppercase letters, @, and :.
                        | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
                        | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                )
            )
        } else {
            execute!(output, PopKeyboardEnhancementFlags)
        }
    }
}

fn suspend_terminal() -> io::Result<()> {
    let keyboard = keyboard_enhancements(&mut io::stdout(), false);
    let focus = execute!(io::stdout(), event::DisableFocusChange);
    let restore = ratatui::try_restore();
    let appearance = execute!(
        io::stdout(),
        SetAttribute(Attribute::Reset),
        ResetColor,
        Show
    );
    keyboard
        .and(focus)
        .and(restore)
        .and(appearance)
        .and(io::stdout().flush())
}

// exec() skips destructors, so finish all TUI cleanup before handing off the tty.
fn restore_terminal(terminal: DefaultTerminal) -> io::Result<()> {
    let result = suspend_terminal();
    drop(terminal);
    result
}

fn check_config(path: &Path) -> io::Result<()> {
    let source = launch::read_config(path)?;
    launch::Config::parse(&source)
        .map_err(|err| io::Error::new(err.kind(), format!("{}: {err}", path.display())))?;
    writeln!(
        io::stdout().lock(),
        "{}: configuration is valid (commands were not executed)",
        path.display()
    )
}

fn read_history(path: &Path) -> io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    const MAX_HISTORY: u64 = 8 * 1024 * 1024;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NONBLOCK);
    }
    let mut file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("history must be a regular file"));
    }
    let offset = metadata.len().saturating_sub(MAX_HISTORY);
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(MAX_HISTORY).read_to_end(&mut bytes)?;
    let start = if offset > 0 {
        bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |index| index + 1)
    } else {
        0
    };
    Ok(String::from_utf8_lossy(&bytes[start..]).into_owned())
}

fn save_history(path: &Path, line: &str, create: bool) -> io::Result<()> {
    if !create && !path.exists() {
        return Ok(());
    }
    let previous = read_history(path).or_else(|err| {
        if create && err.kind() == io::ErrorKind::NotFound {
            Ok(String::new())
        } else {
            Err(err)
        }
    })?;
    if previous.lines().last() == Some(line) {
        return Ok(());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).append(true).create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(nix::libc::O_NONBLOCK);
    }
    let mut file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("history must be a regular file"));
    }
    let mut separator = "";
    if metadata.len() > 0 {
        use std::io::{Read, Seek, SeekFrom};
        file.seek(SeekFrom::End(-1))?;
        let mut last = [0];
        file.read_exact(&mut last)?;
        if last[0] != b'\n' {
            separator = "\n";
        }
    }
    file.write_all(format!("{separator}{line}\n").as_bytes())
}

fn local_username() -> String {
    env::var(if cfg!(windows) { "USERNAME" } else { "USER" }).unwrap_or_default()
}

fn run() -> io::Result<()> {
    let action = cli::parse(env::args_os().skip(1))?;
    match &action {
        cli::Action::DefaultConfig => {
            return write!(io::stdout().lock(), "{}", launch::DEFAULT_CONFIG);
        }
        cli::Action::Help => return writeln!(io::stdout().lock(), "{}", cli::HELP),
        cli::Action::Version => {
            return writeln!(io::stdout().lock(), "shui {}", env!("CARGO_PKG_VERSION"));
        }
        cli::Action::CheckConfig(Some(path)) => return check_config(path),
        _ => {}
    }
    let home = env::home_dir().ok_or_else(|| io::Error::other("home directory is unavailable"))?;
    let home = home.as_path();
    let user = local_username();
    match &action {
        cli::Action::ConfigPath => {
            return writeln!(
                io::stdout().lock(),
                "{}",
                launch::config_path(home).display()
            );
        }
        cli::Action::CheckConfig(None) => return check_config(&launch::config_path(home)),
        cli::Action::Dump => {
            let mut stdout = io::stdout().lock();
            for entry in discover(home, &user) {
                writeln!(stdout, "{}", entry.label())?;
            }
            return Ok(());
        }
        _ => {}
    }
    let cli::Action::Menu {
        query,
        auto_terminal,
    } = action
    else {
        unreachable!()
    };
    cli::require_terminal(auto_terminal)?;
    let mut targets = discover(home, &user);
    let mut config = launch::Config::load(home)?;
    let history = read_history(&home.join(".bash_history")).unwrap_or_default();
    let profiles = config.history_profiles_with_users(
        &targets,
        &history,
        &user,
        &ssh_config::load(home, &user).users,
    );
    let mut state = MenuState {
        query,
        profiles,
        ..MenuState::default()
    };
    loop {
        let mut terminal = init_terminal()?;
        let result = menu(&mut terminal, &mut targets, &mut config, &mut state, home);
        restore_terminal(terminal)?;
        if let Some(Selection {
            target,
            profile: profile_index,
            setup,
            setup_command,
            additional_setup,
            new,
            configure_only,
            connection_command,
            return_to_menu,
        }) = result?
        {
            let target = config.profiles[profile_index]
                .normalize_target(&target, &ssh_config::load(home, &user).users);
            if !configure_only
                && let Some((program, args)) = config.profiles[profile_index].prepare(&target)?
            {
                let status = Command::new(program).args(args).status();
                if !status.as_ref().is_ok_and(|status| status.success()) {
                    state.notice = Some(format!(
                        "preparation cancelled or failed: {}",
                        config.profiles[profile_index].name
                    ));
                    continue;
                }
            }
            let mut setup_failed = false;
            for (index, (program, args)) in
                setup.zip(setup_command).into_iter().chain(additional_setup)
            {
                let script = &config.configuration_scripts[index];
                println!("Running {} for {}", script.name, target.label());
                let status = Command::new(program).args(args).status();
                match &status {
                    Ok(status) => println!("{} finished ({status}).", script.name),
                    Err(error) => eprintln!("Could not run {}: {error}", script.name),
                }
                print!("Press Enter to continue...");
                io::stdout().flush()?;
                io::stdin().read_line(&mut String::new())?;
                if configure_only {
                    setup_failed = !status.as_ref().is_ok_and(|status| status.success());
                    state.notice = Some(match status {
                        Ok(status) if status.success() => {
                            format!("Setup completed: {} · {}", script.name, target.label())
                        }
                        Ok(status) => {
                            format!(
                                "Setup failed ({status}): {} · {}",
                                script.name,
                                target.label()
                            )
                        }
                        Err(err) => format!("Setup failed: {err} · {}", target.label()),
                    });
                    if setup_failed {
                        break;
                    }
                    continue;
                }
                let status = status?;
                if !status.success() {
                    return Err(io::Error::other(format!(
                        "configuration script {} failed ({status}); connection cancelled",
                        script.name
                    )));
                }
            }
            if configure_only || setup_failed {
                continue;
            }
            let custom = connection_command.is_some();
            let (program, args) = match connection_command {
                Some(command) => command,
                None => config.profiles[profile_index].command(&target)?,
            };
            if !custom {
                let line = config.profiles[profile_index].history_line(&target)?;
                if let Err(err) = save_history(&home.join(".bash_history"), &line, new) {
                    eprintln!("shui: could not save connection history: {err}");
                }
            }
            if return_to_menu {
                state.notice = Some(match Command::new(program).args(args).status() {
                    Ok(status) if status.success() => {
                        format!("Connection closed: {}", target.label())
                    }
                    Ok(status) => format!("Connection failed ({status}): {}", target.label()),
                    Err(err) => format!("Connection failed: {err} · {}", target.label()),
                });
                if new && !targets.contains(&target) {
                    targets.insert(0, target);
                    state.profiles.insert(0, profile_index);
                    state.query.clear();
                    state.selection = 0;
                }
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                return Err(Command::new(program).args(args).exec());
            }
            #[cfg(not(unix))]
            {
                let status = Command::new(program).args(args).status()?;
                std::process::exit(status.code().unwrap_or(1));
            }
        }
        return Ok(());
    }
}
fn main() {
    if let Err(err) = run() {
        if err.kind() == io::ErrorKind::BrokenPipe {
            return;
        }
        eprintln!("shui: {err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_protocol_matches_the_platform_backend() {
        let mut output = Vec::new();
        keyboard_enhancements(&mut output, true).unwrap();
        keyboard_enhancements(&mut output, false).unwrap();
        if cfg!(windows) {
            assert!(output.is_empty());
        } else {
            assert_eq!(output, b"\x1b[>15u\x1b[<1u");
        }
    }
    #[test]
    fn edited_text_keeps_the_visible_tail_and_whole_graphemes() {
        for (text, width, expected) in [
            ("abcdef", 4, "…def"),
            ("a界界", 3, "…界"),
            ("abcdefe\u{301}", 3, "…fe\u{301}"),
            ("abc👩\u{200d}💻", 3, "…👩\u{200d}💻"),
            ("abc", 1, "…"),
            ("abc", 0, ""),
            ("abc", 3, "abc"),
        ] {
            let clipped = text_tail(text, width);
            assert_eq!(clipped, expected);
            assert!(Span::raw(clipped).width() <= width);
        }
    }

    #[test]
    fn usernames_are_shown_only_to_disambiguate_unless_configured() {
        let targets: Vec<_> = ["prod", "bob@prod", "alice@other", "bob@other:2222"]
            .iter()
            .map(|name| target(name, "").unwrap())
            .collect();
        let users = HashMap::from([("prod".into(), "alice".into())]);
        assert_eq!(
            display_usernames(&targets, &users, launch::UsernameDisplay::Auto),
            ["alice", "bob", "", ""]
        );
        assert_eq!(
            display_usernames(&targets, &users, launch::UsernameDisplay::Always),
            ["alice", "bob", "alice", "bob"]
        );
        assert_eq!(
            display_usernames(&targets, &users, launch::UsernameDisplay::Never),
            ["", "", "", ""]
        );
    }

    #[test]
    fn normalized_history_keeps_the_selected_profile() {
        let config = launch::Config::parse("default: SSH\nprofiles:\n  - {name: SSH, command: 'ssh %h'}\n  - {name: Mosh, command: 'mosh %h'}\n").unwrap();
        let users = HashMap::from([("prod".into(), "alice".into())]);
        let targets = vec![target("prod", "").unwrap()];
        let history = config.profiles[1]
            .history_line(&target("alice@prod", "").unwrap())
            .unwrap();
        assert_eq!(
            config.history_profiles_with_users(&targets, &history, "alice", &users),
            [1]
        );
        assert_eq!(
            config.history_profiles_with_users(&targets, "mosh alice@prod", "alice", &users),
            [1]
        );
    }

    #[test]
    fn explicit_usernames_and_valid_ports_are_preserved() {
        assert_eq!(target("alice@prod", "alice").unwrap().label(), "alice@prod");
        assert_eq!(
            history_target("ssh -l alice prod", "alice")
                .unwrap()
                .label(),
            "alice@prod"
        );
        for raw in ["prod:0", "prod:65536", "prod:999999999999999999999"] {
            assert!(target(raw, "").is_none());
        }
    }

    #[test]
    fn long_history_tail_stays_bounded_and_new_entries_remain_separate() {
        use std::io::Read;
        let root = env::temp_dir().join(format!("shui-history-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("history");
        let bytes = vec![b'x'; 8 * 1024 * 1024 + 32];
        fs::write(&path, &bytes).unwrap();
        assert_eq!(read_history(&path).unwrap(), "");
        save_history(&path, "ssh prod", false).unwrap();
        assert_eq!(read_history(&path).unwrap(), "ssh prod\n");
        save_history(&path, "ssh prod", false).unwrap();
        let mut tail = String::new();
        fs::File::open(&path)
            .unwrap()
            .take(1)
            .read_to_string(&mut tail)
            .unwrap();
        assert_eq!(tail, "x");
        assert_eq!(fs::metadata(&path).unwrap().len(), bytes.len() as u64 + 10);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn filter_word_deletion() {
        for (input, expected) in [
            ("", ""),
            ("alpha", ""),
            ("alpha beta  ", "alpha "),
            ("alpha привет", "alpha "),
            ("alpha\u{2003}beta", "alpha\u{2003}"),
        ] {
            let mut query = input.to_owned();
            erase_filter_word(&mut query);
            assert_eq!(query, expected);
        }
    }

    #[test]
    fn history_options() {
        let t = history_target("exec ssh -i '/a key' -p 2222 alice@example.org", "alice").unwrap();
        assert_eq!(t.label(), "alice@example.org:2222");
        for delimiter in ["", "-- "] {
            let line = format!("exec ssh -l bob -p2222 {delimiter}host exec bash -l");
            let (target, remote, program) = history_connection(&line, "alice").unwrap();
            assert_eq!(target.label(), "bob@host:2222");
            assert_eq!(remote, ["exec", "bash", "-l"]);
            assert_eq!(program, "ssh");
        }
        assert!(history_target("echo ssh example.org", "alice").is_none());
        assert!(history_target("ssh 'host;touch /tmp/no'", "alice").is_none());
    }
    #[test]
    fn mosh_and_override() {
        let (t, _, program) =
            history_connection("mosh --ssh='ssh -p 2200' bob@host", "alice").unwrap();
        assert_eq!(program, "mosh");
        assert_eq!(t.label(), "bob@host:2200");
    }
    #[test]
    fn filtering() {
        let ts = vec![target("Alice@Host:22", "alice").unwrap()];
        assert_eq!(search::matching(&ts, "HOST", &HashMap::new()), vec![0]);
        assert!(search::matching(&ts, "absent", &HashMap::new()).is_empty());
    }
    #[test]
    fn discovery_recency() {
        let dir = env::temp_dir().join(format!("shui-test-{}", std::process::id()));
        fs::create_dir_all(dir.join(".ssh")).unwrap();
        fs::create_dir_all(dir.join(".config/mc")).unwrap();
        fs::write(
            dir.join(".ssh/config"),
            "Host alpha beta wild*\n User alice\n",
        )
        .unwrap();
        fs::write(
            dir.join(".config/mc/hotlist"),
            "ENTRY \"server\" URL \"/sh://bob@mc/path\"\n",
        )
        .unwrap();
        fs::write(
            dir.join(".bash_history"),
            "ssh alpha\nmosh beta\nssh alpha\n",
        )
        .unwrap();
        let ts = discover(&dir, "alice");
        assert_eq!(ts[0].label(), "alpha");
        assert_eq!(ts[1].label(), "beta");
        assert!(ts.iter().any(|t| t.label() == "bob@mc"));
        assert!(!ts.iter().any(|t| t.host.contains('*')));
        fs::remove_dir_all(dir).unwrap();
    }
}

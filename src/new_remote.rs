use crate::*;
use ratatui::layout::Rect;

pub(super) fn parse_remote(text: &str) -> Option<Target> {
    let text = text.trim();
    if text.starts_with('@') {
        return None;
    }
    let parsed = target(text, "")?;
    if parsed
        .port
        .as_ref()
        .is_some_and(|p| p.parse::<u16>().map_or(true, |p| p == 0))
    {
        return None;
    }
    let host = parsed
        .host
        .rsplit_once('@')
        .map_or(parsed.host.as_str(), |(_, h)| h);
    if host.is_empty()
        || host.starts_with('-')
        || parsed.host.starts_with('@')
        || parsed.host.matches('@').count() > 1
        || (host.contains(':') && !(host.starts_with('[') && host.ends_with(']')))
    {
        return None;
    }
    Some(parsed)
}

pub(super) fn dialog(
    terminal: &mut impl ui::Terminal,
    config: &launch::Config,
    selected: Option<&Target>,
    draft: Option<&Target>,
) -> io::Result<Option<Selection>> {
    setup_form(terminal, config, selected, draft)
}

pub(super) fn connect(
    terminal: &mut impl ui::Terminal,
    config: &launch::Config,
    target: &Target,
    profile: usize,
    fixed_remote: bool,
) -> io::Result<Option<Selection>> {
    form(
        terminal,
        config,
        Some((target, profile)),
        fixed_remote,
        None,
        None,
    )
}

fn next_focus(current: usize, count: usize, remote: usize, fixed: bool, forward: bool) -> usize {
    let step = if forward { 1 } else { count - 1 };
    let next = (current + step) % count;
    if fixed && next == remote {
        (next + step) % count
    } else {
        next
    }
}

fn initial_focus(connecting: bool, configuring: bool, fixed: bool, has_inputs: bool) -> usize {
    let remote = usize::from(!connecting);
    if connecting && has_inputs {
        remote + 2
    } else if configuring || fixed {
        remote + 1
    } else {
        remote
    }
}

fn field_text(label: &str, value: &str, focused: bool, width: usize) -> String {
    let prefix = format!("{label}  ");
    let text = format!("{prefix}{value}");
    if !focused || Line::from(text.as_str()).width() <= width {
        return text;
    }
    let prefix_width = Line::from(prefix.as_str()).width();
    if prefix_width + 4 <= width {
        format!("{prefix}{}", text_tail(value, width - prefix_width))
    } else {
        text_tail(&text, width)
    }
}

struct FormView<'a> {
    connecting: bool,
    configuring: bool,
    fixed_remote: bool,
    remote: &'a str,
    script: Option<&'a launch::Profile>,
    values: Option<&'a HashMap<String, String>>,
    focus: usize,
    generation_error: &'a str,
    error: &'a str,
}

struct Dialog {
    setup_fields: Option<Vec<(String, String)>>,
    setup_input: Option<launch::Input>,
    setup_checkbox: bool,
    queued: Option<(bool, usize)>,
    required_height: usize,
    selector_choices: usize,
    list_state: ListState,
}

impl Dialog {
    fn new(scripts: &[launch::Profile], connecting: bool) -> Self {
        Self {
            setup_fields: None,
            setup_input: None,
            setup_checkbox: false,
            queued: None,
            // Reserve room for every available recipe before opening the form.
            // Switching recipes changes its contents, never its screen position.
            required_height: scripts
                .iter()
                .map(|script| script.inputs.len())
                .max()
                .unwrap_or(0)
                .saturating_add(8 + usize::from(!connecting)),
            selector_choices: scripts.len() + usize::from(!connecting),
            list_state: ListState::default(),
        }
    }

    fn draw(
        &mut self,
        frame: &mut ratatui::Frame,
        config: &launch::Config,
        bar_state: &bar::State,
        view: FormView<'_>,
    ) {
        let FormView {
            connecting,
            configuring,
            fixed_remote,
            remote,
            script,
            values,
            focus,
            generation_error,
            error,
        } = view;
        let inputs = script.map_or(&[][..], |script| script.inputs.as_slice());
        let area = frame.area();
        frame.render_widget(
            Block::default().style(Style::default().fg(theme::TEXT).bg(theme::BASE)),
            area,
        );
        let width = area.width.min(90);
        let height = self.required_height.min(usize::from(area.height)) as u16;
        let rect = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        let areas = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(if height >= 8 { 2 } else { 1 }),
            Constraint::Length(1),
        ])
        .split(rect);
        frame.render_widget(
            Paragraph::new(if connecting {
                " CONNECT "
            } else {
                " REMOTE SETUP "
            })
            .style(
                Style::default()
                    .fg(theme::BASE)
                    .bg(theme::LAVENDER)
                    .add_modifier(Modifier::BOLD),
            ),
            areas[0],
        );
        let mut fields = Vec::new();
        if !connecting {
            fields.push((
                "Action".to_owned(),
                format!(
                    "← {} →",
                    if configuring {
                        "Configure host"
                    } else {
                        "Add new"
                    }
                ),
            ));
        }
        fields.extend([
            ("Remote".to_owned(), remote.to_owned()),
            (
                if connecting { "Profile" } else { "Setup" }.to_owned(),
                if self.selector_choices > 1 {
                    match self.queued {
                        Some((checked, count)) => format!(
                            "← [{}] {} → · {count} queued",
                            if checked { "x" } else { " " },
                            script.map_or("None", |s| s.name.as_str())
                        ),
                        None => format!("← {} →", script.map_or("None", |s| s.name.as_str())),
                    }
                } else {
                    script.map_or("None", |s| s.name.as_str()).to_owned()
                },
            ),
        ]);
        for input in inputs {
            let value = values
                .and_then(|values| values.get(&input.id))
                .map_or("", String::as_str);
            let label = format!(
                "{}{}",
                input.label(),
                if input.required { " *" } else { "" }
            );
            fields.push((
                label,
                if input.kind == launch::InputType::PickString && input.options.len() > 1 {
                    format!("← {value} →")
                } else {
                    value.to_owned()
                },
            ));
        }
        if let Some(setup_fields) = &self.setup_fields {
            fields = setup_fields.clone();
        }
        if fields.len() <= usize::from(areas[1].height) {
            *self.list_state.offset_mut() = 0;
        }
        // A stateful list scrolls the focused field into view on short terminals.
        let items: Vec<_> = fields
            .into_iter()
            .enumerate()
            .map(|(i, (label, value))| {
                ListItem::new(field_text(
                    &label,
                    &value,
                    i == focus,
                    usize::from(areas[1].width.saturating_sub(2)),
                ))
            })
            .collect();
        let list = List::new(items)
            .style(Style::default().bg(theme::SURFACE))
            .highlight_symbol("› ")
            .highlight_style(Style::default().bg(theme::SELECTION));
        self.list_state.select(Some(focus));
        frame.render_stateful_widget(list, areas[1], &mut self.list_state);
        frame.render_widget(
            Paragraph::new(if !generation_error.is_empty() {
                generation_error
            } else if error.is_empty() {
                if configuring {
                    " Runs setup only · returns to menu · * required"
                } else if fixed_remote {
                    " Remote fixed for setup · * required"
                } else {
                    " user@host[:port] · * required"
                }
            } else {
                error
            })
            .wrap(ratatui::widgets::Wrap { trim: false })
            .style(Style::default().fg(
                if error.is_empty() && generation_error.is_empty() {
                    theme::SUBTEXT
                } else {
                    theme::PEACH
                },
            )),
            areas[2],
        );
        let keys = &config.keybindings;
        let remote_field = usize::from(!connecting);
        let input = if self.setup_fields.is_some() {
            self.setup_input.as_ref()
        } else {
            focus
                .checked_sub(remote_field + 2)
                .and_then(|index| inputs.get(index))
        };
        let can_choose = (!connecting && focus == 0)
            || (self.setup_fields.is_none()
                && focus == remote_field + 1
                && self.selector_choices > 1)
            || input.is_some_and(|input| {
                input.kind == launch::InputType::PickString && input.options.len() > 1
            });
        let can_edit = (focus == remote_field && !fixed_remote)
            || input.is_some_and(|input| input.kind == launch::InputType::PromptString);
        let mut items = vec![
            bar::Item::action(keys, "dialog", "toggle_recipe", "Queue", 2).available(
                if self.setup_fields.is_some() {
                    self.setup_checkbox
                } else {
                    !connecting && focus == remote_field + 1
                },
            ),
            bar::Item::pair(keys, "dialog", "previous_field", "next_field", "Field", 3),
            bar::Item::pair(keys, "dialog", "left", "right", "Choice", 4).available(can_choose),
            bar::Item::action(
                keys,
                "dialog",
                "connect",
                if connecting {
                    "Open"
                } else if configuring {
                    "Apply"
                } else {
                    "Add+open"
                },
                2,
            ),
            bar::Item::action(
                keys,
                "dialog",
                "connect_and_return",
                if configuring { "Apply" } else { "Return" },
                5,
            ),
            bar::Item::action(keys, "dialog", "help", "Help", 0),
            bar::Item::action(keys, "dialog", "cancel", "Cancel", 1),
        ];
        // Pair hints describe both directions, but modified menus enumerate the
        // underlying actions independently. Suppress unavailable actions there too.
        for (actions, available) in [
            (&["left", "right"][..], can_choose),
            (&["erase_character", "erase_word"][..], can_edit),
        ] {
            if !available {
                items.extend(actions.iter().map(|action| {
                    bar::Item::action(keys, "dialog", action, "", 10).available(false)
                }));
            }
        }
        bar_state.draw(frame, areas[3], config, "dialog", items);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SetupRow {
    Action,
    Remote,
    Recipe(usize),
    Input(usize, usize),
}

fn setup_form(
    terminal: &mut impl ui::Terminal,
    config: &launch::Config,
    selected: Option<&Target>,
    draft: Option<&Target>,
) -> io::Result<Option<Selection>> {
    let available: Vec<_> = config
        .configuration_scripts
        .iter()
        .enumerate()
        .filter(|(_, recipe)| recipe.is_available())
        .map(|(i, _)| i)
        .collect();
    let mut recipes: Vec<_> = available
        .iter()
        .map(|i| config.configuration_scripts[*i].clone())
        .collect();
    let mut checked = vec![false; recipes.len()];
    let mut loaded = vec![false; recipes.len()];
    let mut errors = vec![String::new(); recipes.len()];
    let mut values: Vec<_> = recipes.iter().map(|r| r.input_defaults()).collect();
    let mut remote = selected.or(draft).map(Target::label).unwrap_or_default();
    let mut alternate_remote = String::new();
    let mut configuring = selected.is_some();
    let mut focus = if configuring && !recipes.is_empty() {
        SetupRow::Recipe(0)
    } else {
        SetupRow::Remote
    };
    let mut error = String::new();
    let mut bar_state = bar::State::default();
    let mut dialog = Dialog::new(&recipes, false);
    dialog.required_height = 8 + recipes.iter().map(|r| 1 + r.inputs.len()).sum::<usize>();
    loop {
        let mut rows = vec![SetupRow::Action, SetupRow::Remote];
        let mut fields = vec![
            (
                "Action".into(),
                format!(
                    "← {} →",
                    if configuring {
                        "Configure host"
                    } else {
                        "Add new"
                    }
                ),
            ),
            ("Remote".into(), remote.clone()),
        ];
        for (i, recipe) in recipes.iter().enumerate() {
            rows.push(SetupRow::Recipe(i));
            fields.push((
                format!("[{}]", if checked[i] { "x" } else { " " }),
                recipe.name.clone(),
            ));
            if checked[i] {
                for (j, input) in recipe.inputs.iter().enumerate() {
                    rows.push(SetupRow::Input(i, j));
                    let value = values[i].get(&input.id).cloned().unwrap_or_default();
                    fields.push((
                        format!(
                            "  {}{}",
                            input.label(),
                            if input.required { " *" } else { "" }
                        ),
                        if input.kind == launch::InputType::PickString && input.options.len() > 1 {
                            format!("← {value} →")
                        } else {
                            value
                        },
                    ));
                }
            }
        }
        let position = rows.iter().position(|r| *r == focus).unwrap_or(1);
        focus = rows[position];
        dialog.setup_fields = Some(fields);
        dialog.setup_checkbox = matches!(focus, SetupRow::Recipe(_));
        dialog.setup_input = match focus {
            SetupRow::Input(i, j) => Some(recipes[i].inputs[j].clone()),
            _ => None,
        };
        let generation_error = match focus {
            SetupRow::Recipe(i) | SetupRow::Input(i, _) if checked[i] => errors[i].as_str(),
            _ => "",
        };
        terminal.draw(|frame| {
            dialog.draw(
                frame,
                config,
                &bar_state,
                FormView {
                    connecting: false,
                    configuring,
                    fixed_remote: false,
                    remote: &remote,
                    script: None,
                    values: None,
                    focus: position,
                    generation_error,
                    error: &error,
                },
            )
        })?;
        let event = terminal.read()?;
        if matches!(event, Event::FocusLost) {
            bar_state.reset();
        }
        let Event::Key(event) = event else {
            continue;
        };
        if bar_state.event(event, &config.keybindings, "dialog")
            || event.kind == KeyEventKind::Release
        {
            continue;
        }
        let repeated = event.kind == KeyEventKind::Repeat;
        let Some(key) = config
            .keybindings
            .translate_dialog(event, dialog.setup_checkbox)
        else {
            continue;
        };
        if repeated && matches!(key.code, KeyCode::Enter | KeyCode::F(_) | KeyCode::Esc) {
            continue;
        }
        match key.code {
            KeyCode::Esc => return Ok(None),
            KeyCode::F(1) => {
                bar_state.reset();
                crate::help::show(terminal, crate::help::Context::Setup, config)?;
            }
            KeyCode::Tab | KeyCode::Down => focus = rows[(position + 1) % rows.len()],
            KeyCode::BackTab | KeyCode::Up => {
                focus = rows[(position + rows.len() - 1) % rows.len()]
            }
            KeyCode::F(6) => {
                if let SetupRow::Recipe(i) = focus {
                    checked[i] = !checked[i];
                    error.clear();
                    if checked[i] && !loaded[i] {
                        match recipes[i]
                            .inputs
                            .iter()
                            .map(|input| input.resolve())
                            .collect::<io::Result<Vec<_>>>()
                        {
                            Ok(inputs) => {
                                recipes[i].inputs = inputs;
                                values[i] = recipes[i].input_defaults();
                                loaded[i] = true;
                                errors[i].clear();
                            }
                            Err(err) => errors[i] = err.to_string(),
                        }
                    }
                }
            }
            KeyCode::Left | KeyCode::Right => {
                match focus {
                    SetupRow::Action => {
                        configuring = !configuring;
                        std::mem::swap(&mut remote, &mut alternate_remote);
                    }
                    SetupRow::Input(i, j) => {
                        let input = &recipes[i].inputs[j];
                        if input.kind == launch::InputType::PickString && !input.options.is_empty()
                        {
                            let value = values[i].get_mut(&input.id).unwrap();
                            let index = input
                                .options
                                .iter()
                                .position(|option| option == value)
                                .unwrap_or(0);
                            let delta = if key.code == KeyCode::Right {
                                1
                            } else {
                                input.options.len() - 1
                            };
                            *value = input.options[(index + delta) % input.options.len()].clone();
                        }
                    }
                    _ => {}
                }
                error.clear();
            }
            KeyCode::Enter => {
                if configuring && !checked.iter().any(|v| *v) {
                    error = " Choose a setup recipe to configure this host".into();
                    if !recipes.is_empty() {
                        focus = SetupRow::Recipe(0);
                    }
                    continue;
                }
                let Some(target) = parse_remote(&remote) else {
                    error = " Enter a valid host and optional port (1–65535)".into();
                    focus = SetupRow::Remote;
                    continue;
                };
                let target = if let Some(home) = env::home_dir() {
                    let users = ssh_config::load(&home, &local_username()).users;
                    if configuring {
                        ssh_config::normalize_target(&target, &users)
                    } else {
                        config.profiles[config.default_index()].normalize_target(&target, &users)
                    }
                } else {
                    target
                };
                let mut commands = Vec::new();
                let mut invalid = false;
                for (i, recipe) in recipes.iter().enumerate() {
                    if !checked[i] {
                        continue;
                    }
                    if !errors[i].is_empty() {
                        focus = SetupRow::Recipe(i);
                        invalid = true;
                        break;
                    }
                    if let Some((j, err)) =
                        recipe.inputs.iter().enumerate().find_map(|(j, input)| {
                            input
                                .validate_value(&values[i][&input.id])
                                .err()
                                .map(|err| (j, err))
                        })
                    {
                        error = format!(" {}: {err}", recipe.name);
                        focus = SetupRow::Input(i, j);
                        invalid = true;
                        break;
                    }
                    match recipe.command_with_inputs(&target, &values[i]) {
                        Ok(command) => commands.push((available[i], command)),
                        Err(err) => {
                            error = format!(" {}: {err}", recipe.name);
                            focus = SetupRow::Recipe(i);
                            invalid = true;
                            break;
                        }
                    }
                }
                if invalid {
                    continue;
                }
                let mut commands = commands.into_iter();
                let first = commands.next();
                return Ok(Some(Selection {
                    target,
                    profile: config.default_index(),
                    setup: first.as_ref().map(|(i, _)| *i),
                    setup_command: first.map(|(_, command)| command),
                    additional_setup: commands.collect(),
                    new: !configuring,
                    configure_only: configuring,
                    return_to_menu: key.modifiers.contains(KeyModifiers::ALT),
                    connection_command: None,
                }));
            }
            _ => {
                let editable = match focus {
                    SetupRow::Remote => Some(&mut remote),
                    SetupRow::Input(i, j)
                        if recipes[i].inputs[j].kind == launch::InputType::PromptString =>
                    {
                        values[i].get_mut(&recipes[i].inputs[j].id)
                    }
                    _ => None,
                };
                if let Some(value) = editable {
                    match key.code {
                        KeyCode::Backspace | KeyCode::Delete | KeyCode::Char('h')
                            if key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            crate::erase_filter_word(value)
                        }
                        KeyCode::Backspace | KeyCode::Delete => {
                            value.pop();
                        }
                        KeyCode::Char(c)
                            if !c.is_control()
                                && !key
                                    .modifiers
                                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                        {
                            value.push(c)
                        }
                        _ => continue,
                    }
                    error.clear();
                }
            }
        }
    }
}

fn form(
    terminal: &mut impl ui::Terminal,
    config: &launch::Config,
    connection: Option<(&Target, usize)>,
    fixed_remote: bool,
    selected: Option<&Target>,
    draft: Option<&Target>,
) -> io::Result<Option<Selection>> {
    let connecting = connection.is_some();
    let mut configuring = selected.is_some();
    let mut remote = connection
        .map(|(t, _)| t.label())
        .or_else(|| selected.or(draft).map(Target::label))
        .unwrap_or_default();
    let mut alternate_remote = String::new();
    let remote_field = usize::from(!connecting);
    let recipe_field = remote_field + 1;
    let input_start = recipe_field + 1;
    let mut setup = connection.map_or(0, |(_, p)| p + 1);
    let available: Vec<_> = if connecting {
        (0..config.profiles.len()).collect()
    } else {
        config
            .configuration_scripts
            .iter()
            .enumerate()
            .filter(|(_, script)| script.is_available())
            .map(|(i, _)| i)
            .collect()
    };
    let mut scripts: Vec<_> = available
        .iter()
        .map(|i| {
            if connecting {
                config.profiles[*i].clone()
            } else {
                config.configuration_scripts[*i].clone()
            }
        })
        .collect();
    let mut focus = initial_focus(
        connecting,
        configuring,
        fixed_remote,
        connecting && !scripts[setup - 1].inputs.is_empty(),
    );
    let mut dialog = Dialog::new(&scripts, connecting);
    let mut values: Vec<_> = scripts.iter().map(|p| p.input_defaults()).collect();
    let mut loaded = vec![false; scripts.len()];
    let mut queued = vec![false; scripts.len()];
    let mut queue_mode = false;
    let mut generation_error = String::new();
    let mut error = String::new();
    let mut bar_state = bar::State::default();
    loop {
        if let Some(i) = setup.checked_sub(1)
            && !loaded[i]
            && generation_error.is_empty()
        {
            let resolved: io::Result<Vec<_>> = scripts[i]
                .inputs
                .iter()
                .map(|input| input.resolve())
                .collect();
            match resolved {
                Ok(inputs) => {
                    scripts[i].inputs = inputs;
                    values[i] = scripts[i].input_defaults();
                    loaded[i] = true;
                }
                Err(err) => generation_error = err.to_string(),
            }
        }
        let script = setup.checked_sub(1).map(|i| &scripts[i]);
        let inputs = script.map_or(&[][..], |s| s.inputs.as_slice());
        let field_count = inputs.len() + input_start;
        focus = focus.min(field_count - 1);
        dialog.queued = (!connecting && queue_mode).then(|| {
            (
                setup.checked_sub(1).is_some_and(|i| queued[i]),
                queued.iter().filter(|selected| **selected).count(),
            )
        });
        terminal.draw(|frame| {
            dialog.draw(
                frame,
                config,
                &bar_state,
                FormView {
                    connecting,
                    configuring,
                    fixed_remote,
                    remote: &remote,
                    script,
                    values: setup.checked_sub(1).map(|index| &values[index]),
                    focus,
                    generation_error: &generation_error,
                    error: &error,
                },
            );
        })?;
        let event = terminal.read()?;
        if matches!(event, Event::FocusLost) {
            bar_state.reset();
        }
        let Event::Key(key) = event else {
            continue;
        };
        if bar_state.event(key, &config.keybindings, "dialog") {
            continue;
        }
        if key.kind == KeyEventKind::Release {
            continue;
        }
        let repeated = key.kind == KeyEventKind::Repeat;
        let Some(key) = config
            .keybindings
            .translate_dialog(key, !connecting && focus == recipe_field)
        else {
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
                    | KeyCode::Tab
                    | KeyCode::BackTab
            )
        {
            continue;
        }
        match key.code {
            KeyCode::F(6) if !connecting && focus == recipe_field => {
                if !repeated {
                    queue_mode = true;
                    if let Some(i) = setup.checked_sub(1) {
                        queued[i] = !queued[i];
                    } else {
                        queued.fill(false);
                    }
                    error.clear();
                }
            }
            KeyCode::F(1) => {
                bar_state.reset();
                crate::help::show(
                    terminal,
                    if connecting {
                        crate::help::Context::Connection
                    } else {
                        crate::help::Context::Setup
                    },
                    config,
                )?;
            }
            KeyCode::Esc => return Ok(None),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(None);
            }
            KeyCode::Tab | KeyCode::Down => {
                focus = next_focus(focus, field_count, remote_field, fixed_remote, true);
            }
            KeyCode::BackTab | KeyCode::Up => {
                focus = next_focus(focus, field_count, remote_field, fixed_remote, false);
            }
            KeyCode::Left | KeyCode::Right => {
                let next = |current: usize, len: usize| {
                    if key.code == KeyCode::Right {
                        (current + 1) % len
                    } else {
                        (current + len - 1) % len
                    }
                };
                if !connecting && focus == 0 {
                    configuring = !configuring;
                    std::mem::swap(&mut remote, &mut alternate_remote);
                } else if focus == recipe_field {
                    setup = if connecting {
                        next(setup - 1, scripts.len()) + 1
                    } else {
                        next(setup, scripts.len() + 1)
                    };
                    generation_error.clear();
                } else if focus >= input_start {
                    let input = &inputs[focus - input_start];
                    if input.kind == launch::InputType::PickString && !input.options.is_empty() {
                        let value = values[setup - 1].get_mut(&input.id).unwrap();
                        let index = input.options.iter().position(|v| v == value).unwrap_or(0);
                        *value = input.options[next(index, input.options.len())].clone();
                    }
                }
                error.clear();
            }
            KeyCode::Enter => {
                if !generation_error.is_empty()
                    && (connecting
                        || !queue_mode
                        || setup.checked_sub(1).is_some_and(|i| queued[i]))
                {
                    continue;
                }
                if configuring
                    && if queue_mode {
                        !queued.iter().any(|v| *v)
                    } else {
                        setup == 0
                    }
                {
                    error = " Choose a setup recipe to configure this host".into();
                    focus = recipe_field;
                    continue;
                }
                let Some(target) = parse_remote(&remote) else {
                    error = " Enter a valid host and optional port (1–65535)".into();
                    focus = remote_field;
                    continue;
                };
                let target = if let Some(home) = env::home_dir() {
                    let users = ssh_config::load(Path::new(&home), &local_username()).users;
                    if configuring {
                        ssh_config::normalize_target(&target, &users)
                    } else {
                        config.profiles[if connecting {
                            setup - 1
                        } else {
                            config.default_index()
                        }]
                        .normalize_target(&target, &users)
                    }
                } else {
                    target
                };
                let selected_values = setup
                    .checked_sub(1)
                    .map_or_else(HashMap::new, |i| values[i].clone());
                let mut setup_command = None;
                let mut additional_setup = Vec::new();
                let mut first_setup = None;
                if !connecting && queue_mode {
                    let mut invalid = false;
                    for i in 0..scripts.len() {
                        if !queued[i] {
                            continue;
                        }
                        if !loaded[i] {
                            setup = i + 1;
                            generation_error.clear();
                            invalid = true;
                            break;
                        }
                        if let Some((index, err)) =
                            scripts[i]
                                .inputs
                                .iter()
                                .enumerate()
                                .find_map(|(index, input)| {
                                    input
                                        .validate_value(&values[i][&input.id])
                                        .err()
                                        .map(|err| (index, err))
                                })
                        {
                            error = format!(" {}: {err}", scripts[i].name);
                            setup = i + 1;
                            focus = index + input_start;
                            invalid = true;
                            break;
                        }
                        match scripts[i].command_with_inputs(&target, &values[i]) {
                            Ok(command) => {
                                if first_setup.is_none() {
                                    first_setup = Some(available[i]);
                                    setup_command = Some(command);
                                } else {
                                    additional_setup.push((available[i], command));
                                }
                            }
                            Err(err) => {
                                error = format!(" {}: {err}", scripts[i].name);
                                setup = i + 1;
                                invalid = true;
                                break;
                            }
                        }
                    }
                    if invalid {
                        continue;
                    }
                } else if let Some(script) = script {
                    if let Some((index, err)) =
                        inputs.iter().enumerate().find_map(|(index, input)| {
                            input
                                .validate_value(&selected_values[&input.id])
                                .err()
                                .map(|err| (index, err))
                        })
                    {
                        error = format!(" {err}");
                        focus = index + input_start;
                        continue;
                    }
                    match script.command_with_inputs(&target, &selected_values) {
                        Ok(command) => setup_command = Some(command),
                        Err(err) => {
                            error = format!(" {err}");
                            continue;
                        }
                    }
                }
                return Ok(Some(Selection {
                    target,
                    profile: if connecting {
                        setup - 1
                    } else {
                        config.default_index()
                    },
                    setup: if connecting {
                        None
                    } else if queue_mode {
                        first_setup
                    } else {
                        setup.checked_sub(1).map(|i| available[i])
                    },
                    connection_command: if connecting {
                        setup_command.clone()
                    } else {
                        None
                    },
                    setup_command: if connecting { None } else { setup_command },
                    additional_setup,
                    new: !connecting && !configuring,
                    configure_only: configuring,
                    return_to_menu: key.modifiers.contains(KeyModifiers::ALT),
                }));
            }
            _ => {
                let editable = if focus == remote_field && !fixed_remote {
                    Some(&mut remote)
                } else if focus >= input_start
                    && inputs[focus - input_start].kind == launch::InputType::PromptString
                {
                    values[setup - 1].get_mut(&inputs[focus - input_start].id)
                } else {
                    None
                };
                if let Some(value) = editable {
                    match key.code {
                        KeyCode::Backspace | KeyCode::Delete | KeyCode::Char('h')
                            if key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            crate::erase_filter_word(value)
                        }
                        KeyCode::Backspace | KeyCode::Delete => {
                            value.pop();
                        }
                        KeyCode::Char(c)
                            if !c.is_control()
                                && !key
                                    .modifiers
                                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                        {
                            value.push(c)
                        }
                        _ => continue,
                    }
                    error.clear();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn sample_config() -> launch::Config {
        let inputs = (0..7)
            .map(|index| format!("      - {{id: field{index}, type: promptString, description: Field{index}, default: value{index}}}"))
            .collect::<Vec<_>>()
            .join("\n");
        launch::Config::parse(&format!(
            "default: Zero\nprofiles:\n  - {{name: Zero, command: 'ssh %h'}}\n  - name: One\n    command: ssh %h\n    inputs:\n      - {{id: single, type: promptString, default: example}}\n  - name: Many\n    command: ssh %h\n    inputs:\n{inputs}\n"
        )).unwrap()
    }

    fn render(
        terminal: &mut Terminal<TestBackend>,
        dialog: &mut Dialog,
        config: &launch::Config,
        view: FormView<'_>,
    ) -> Vec<String> {
        render_with_bar(terminal, dialog, config, &bar::State::default(), view)
    }

    fn render_with_bar(
        terminal: &mut Terminal<TestBackend>,
        dialog: &mut Dialog,
        config: &launch::Config,
        bar_state: &bar::State,
        view: FormView<'_>,
    ) -> Vec<String> {
        terminal
            .draw(|frame| dialog.draw(frame, config, bar_state, view))
            .unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content
            .chunks(usize::from(buffer.area.width))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect()
    }

    fn position(lines: &[String], text: &str) -> (usize, usize) {
        lines
            .iter()
            .enumerate()
            .find_map(|(y, line)| line.find(text).map(|x| (Line::from(&line[..x]).width(), y)))
            .unwrap_or_else(|| panic!("missing {text:?}: {lines:?}"))
    }

    #[test]
    fn rendered_choices_and_modified_edit_hints_follow_the_focused_field() {
        let config = launch::Config::parse(
            "default: Only\nprofiles:\n  - name: Only\n    command: ssh %h\n    inputs:\n      - {id: text, type: promptString, default: text}\n      - {id: single, type: pickString, options: [one]}\n      - {id: multiple, type: pickString, options: [one, two]}\nkeybindings:\n  dialog:\n    left: [Left, Ctrl+Left]\n    right: [Right, Ctrl+Right]\n    erase_character: [Backspace, Delete, Alt+Backspace]\n",
        )
        .unwrap();
        let script = &config.profiles[0];
        let values = script.input_defaults();
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        let mut dialog = Dialog::new(&config.profiles, true);
        for (focus, fixed_remote, can_choose, can_edit) in [
            (0, false, false, true),
            (0, true, false, false),
            (1, false, false, false),
            (2, false, false, true),
            (3, false, false, false),
            (4, false, true, false),
        ] {
            for modifier in [KeyModifiers::NONE, KeyModifiers::CONTROL, KeyModifiers::ALT] {
                let mut bar_state = bar::State::default();
                if !modifier.is_empty() {
                    let code = if modifier == KeyModifiers::CONTROL {
                        crossterm::event::ModifierKeyCode::LeftControl
                    } else {
                        crossterm::event::ModifierKeyCode::LeftAlt
                    };
                    bar_state.event(
                        crossterm::event::KeyEvent::new(KeyCode::Modifier(code), modifier),
                        &config.keybindings,
                        "dialog",
                    );
                }
                let lines = render_with_bar(
                    &mut terminal,
                    &mut dialog,
                    &config,
                    &bar_state,
                    FormView {
                        connecting: true,
                        configuring: false,
                        fixed_remote,
                        remote: "example.org",
                        script: Some(script),
                        values: Some(&values),
                        focus,
                        generation_error: "",
                        error: "",
                    },
                );
                for label in ["Profile", "single"] {
                    let row = &lines[position(&lines, label).1];
                    assert!(!row.contains(['←', '→']), "{row}");
                }
                assert!(lines[position(&lines, "multiple").1].contains("← one →"));
                let footer = lines
                    .iter()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .unwrap();
                match modifier {
                    KeyModifiers::NONE => {
                        assert_eq!(footer.contains("Choice"), can_choose, "{footer}")
                    }
                    KeyModifiers::CONTROL => {
                        assert_eq!(footer.contains("Previous"), can_choose, "{footer}");
                        assert_eq!(footer.contains("Next"), can_choose, "{footer}");
                        assert_eq!(footer.contains("Word"), can_edit, "{footer}");
                    }
                    KeyModifiers::ALT => assert_eq!(footer.contains("Erase"), can_edit, "{footer}"),
                    _ => unreachable!(),
                }
            }
        }

        // A setup form without any recipes has only None; Action still offers a choice.
        let mut dialog = Dialog::new(&[], false);
        for focus in [0, 2] {
            let lines = render(
                &mut terminal,
                &mut dialog,
                &config,
                FormView {
                    connecting: false,
                    configuring: false,
                    fixed_remote: false,
                    remote: "example.org",
                    script: None,
                    values: None,
                    focus,
                    generation_error: "",
                    error: "",
                },
            );
            let selector = &lines[position(&lines, "Setup").1];
            assert!(selector.contains("None"));
            assert!(!selector.contains(['←', '→']), "{selector}");
            assert_eq!(lines.iter().any(|line| line.contains("Choice")), focus == 0);
        }
    }

    #[test]
    fn rendered_dialog_anchors_stay_fixed_across_recipes_focus_and_errors() {
        let config = sample_config();
        for connecting in [false, true] {
            let mut terminal = Terminal::new(TestBackend::new(120, 32)).unwrap();
            let mut dialog = Dialog::new(&config.profiles, connecting);
            let mut anchors = None;
            for selected in [None, Some(0), Some(1), Some(2), Some(0), None] {
                let script = selected.map(|index| &config.profiles[index]);
                let values = script.map(launch::Profile::input_defaults);
                let fields =
                    script.map_or(0, |script| script.inputs.len()) + 2 + usize::from(!connecting);
                for focus in 0..fields {
                    for (error, generation_error) in [
                        ("", ""),
                        (" Enter a valid host and optional port (1–65535)", ""),
                        ("", "Generator failed: please choose another recipe"),
                    ] {
                        let lines = render(
                            &mut terminal,
                            &mut dialog,
                            &config,
                            FormView {
                                connecting,
                                configuring: !connecting,
                                fixed_remote: false,
                                remote: "deploy@example.org:2222",
                                script,
                                values: values.as_ref(),
                                focus,
                                generation_error,
                                error,
                            },
                        );
                        let current = [
                            position(
                                &lines,
                                if connecting {
                                    "CONNECT"
                                } else {
                                    "REMOTE SETUP"
                                },
                            ),
                            position(&lines, "Remote"),
                            position(&lines, if connecting { "Profile" } else { "Setup" }),
                            // Contextual hints can shorten the footer horizontally.
                            (0, position(&lines, "F1 Help").1),
                        ];
                        if let Some(expected) = anchors {
                            assert_eq!(current, expected);
                        } else {
                            anchors = Some(current);
                        }
                        assert_eq!(dialog.list_state.offset(), 0);
                    }
                }
            }
        }
    }

    #[test]
    fn short_dialog_scroll_remains_stable_until_focus_leaves_the_viewport() {
        let config = sample_config();
        let script = &config.profiles[2];
        let values = script.input_defaults();
        let mut terminal = Terminal::new(TestBackend::new(32, 8)).unwrap();
        let mut dialog = Dialog::new(&config.profiles, true);
        let mut draw = |focus| {
            render(
                &mut terminal,
                &mut dialog,
                &config,
                FormView {
                    connecting: true,
                    configuring: false,
                    fixed_remote: true,
                    remote: "example.org",
                    script: Some(script),
                    values: Some(&values),
                    focus,
                    generation_error: "",
                    error: "",
                },
            )
        };
        let last = draw(8);
        let previous = draw(7);
        assert_eq!(position(&last, "Field3"), position(&previous, "Field3"));
        assert_eq!(position(&last, "Field6"), position(&previous, "Field6"));
        assert_eq!(position(&last, "CONNECT"), position(&previous, "CONNECT"));
        assert_eq!(position(&last, "F1 Help"), position(&previous, "F1 Help"));
        assert_eq!(dialog.list_state.offset(), 5);

        terminal.backend_mut().resize(80, 24);
        terminal.resize(Rect::new(0, 0, 80, 24)).unwrap();
        let lines = render(
            &mut terminal,
            &mut dialog,
            &config,
            FormView {
                connecting: true,
                configuring: false,
                fixed_remote: true,
                remote: "example.org",
                script: Some(script),
                values: Some(&values),
                focus: 7,
                generation_error: "",
                error: "",
            },
        );
        assert_eq!(dialog.list_state.offset(), 0);
        assert!(position(&lines, "Remote").1 < position(&lines, "Field0").1);
    }

    #[test]
    fn focused_values_keep_labels_and_unicode_tails_visible() {
        let config = sample_config();
        let script = &config.profiles[1];
        let values = HashMap::from([("single".into(), "prefix-long-e\u{301}-界界界尾".into())]);
        let mut terminal = Terminal::new(TestBackend::new(22, 8)).unwrap();
        let mut dialog = Dialog::new(&config.profiles, true);
        let lines = render(
            &mut terminal,
            &mut dialog,
            &config,
            FormView {
                connecting: true,
                configuring: false,
                fixed_remote: false,
                remote: "example.org",
                script: Some(script),
                values: Some(&values),
                focus: 2,
                generation_error: "",
                error: "",
            },
        );
        let selected = lines.iter().find(|line| line.contains('›')).unwrap();
        assert!(selected.contains("single"), "{selected}");
        assert!(selected.contains('…'), "{selected}");
        assert!(selected.contains('尾'), "{selected}");
    }

    #[test]
    fn tiny_forms_render_and_fixed_remote_navigation_never_selects_locked_field() {
        let config = sample_config();
        for (width, height) in [(1, 1), (12, 3), (20, 5), (32, 8)] {
            for connecting in [false, true] {
                for index in 0..config.profiles.len() {
                    let script = &config.profiles[index];
                    let values = script.input_defaults();
                    let remote = usize::from(!connecting);
                    let count = remote + 2 + script.inputs.len();
                    for fixed in [false, true] {
                        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                        let mut dialog = Dialog::new(&config.profiles, connecting);
                        let mut focus = initial_focus(
                            connecting,
                            !connecting,
                            fixed,
                            !script.inputs.is_empty(),
                        );
                        for forward in [true, false] {
                            for _ in 0..count + 1 {
                                if fixed {
                                    assert_ne!(focus, remote);
                                }
                                render(
                                    &mut terminal,
                                    &mut dialog,
                                    &config,
                                    FormView {
                                        connecting,
                                        configuring: !connecting,
                                        fixed_remote: fixed,
                                        remote: "example.org",
                                        script: Some(script),
                                        values: Some(&values),
                                        focus,
                                        generation_error: "",
                                        error: "A validation error that must not move this form",
                                    },
                                );
                                focus = next_focus(focus, count, remote, fixed, forward);
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(initial_focus(false, false, false, false), 1);
        assert_eq!(initial_focus(false, true, false, false), 2);
        assert_eq!(initial_focus(true, false, false, false), 0);
        assert_eq!(initial_focus(true, false, true, false), 1);
        assert_eq!(initial_focus(true, false, false, true), 2);
        assert_eq!(initial_focus(true, false, true, true), 2);
    }

    #[test]
    fn validates_remote_before_running_scripts() {
        for valid in ["host", "alice@host:2222", "user@[::1]:22"] {
            assert!(parse_remote(valid).is_some(), "{valid}");
        }
        for invalid in [
            "",
            "-oProxyCommand=x",
            "host;touch x",
            "@host",
            "user@",
            "host:0",
            "host:65536",
            "host:abc",
            "a@b@c",
        ] {
            assert!(parse_remote(invalid).is_none(), "{invalid}");
        }
    }
}

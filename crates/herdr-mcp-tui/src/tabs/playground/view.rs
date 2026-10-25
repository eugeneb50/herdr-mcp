//! Drawing the Playground: the collection tree, the picked collection or
//! request, the response, and the pickers drawn over them.

use herdr_mcp_api::VarScope;
use herdr_mcp_api::gate::rule_text;
use herdr_mcp_api::playground::{
    BodyMode, HTTP_METHODS, KeyMode, PROVIDERS, display_value, json_body_error, model_choices,
    tool_label,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use super::{
    CRow, Focus, HttpRow, LlmRow, Overlay, Pick, RRow, RequestForm, Target, TrailField, TreeRow,
    View, extract_text, kv_text, method_choices, request_count, tool_choices, tree_rows,
    trigger_text,
};
use crate::App;
use crate::theme::{
    accent_style, dim_style, focused_panel_block, muted_style, panel_block, selected_style,
    warn_style,
};

fn block(title: &str, focused: bool) -> Block<'static> {
    if focused {
        focused_panel_block(title)
    } else {
        panel_block(title)
    }
}

/// A list line: `▸` and the selection style when `on`.
fn item(spans: Vec<Span<'static>>, on: bool) -> ListItem<'static> {
    let mut all = vec![Span::styled(if on { "▸ " } else { "  " }, accent_style())];
    all.extend(spans);
    let line = Line::from(all);
    ListItem::new(if on {
        line.style(selected_style())
    } else {
        line
    })
}

fn heading(text: String) -> ListItem<'static> {
    ListItem::new(Line::styled(
        text,
        accent_style().add_modifier(Modifier::BOLD),
    ))
}

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let pg = &app.playground;
    let mut spans = vec![
        Span::styled("Playground", accent_style().add_modifier(Modifier::BOLD)),
        Span::styled(
            match pg.view {
                View::Collections => "  Collections",
                View::Events => "  herdr Events",
                View::Runs => "  Runs",
            },
            dim_style(),
        ),
    ];
    spans.push(Span::styled(
        match pg.view {
            View::Collections => {
                "   Tab panel  Enter open/edit  Ctrl+S save  Ctrl+G send/run  Ctrl+N new  Ctrl+E events"
            }
            View::Events => "   type to filter  Esc clear filter  Ctrl+D clear  Ctrl+E collections",
            View::Runs => "   ↑/↓ pick  f failed only  c next collection  Esc collections",
        },
        dim_style(),
    ));
    if !app.bridge_connected {
        spans.push(Span::styled("   not connected", warn_style()));
    }
    match &pg.msg {
        Some(Ok(m)) => spans.push(Span::styled(format!("   {m}"), accent_style())),
        Some(Err(m)) => spans.push(Span::styled(format!("   ⚠ {m}"), warn_style())),
        None => {}
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(area.x, area.y, area.width, 1),
    );
    let body = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );
    if app.playground.view == View::Events {
        render_events(frame, body, app);
        return;
    }
    if app.playground.view == View::Runs {
        super::runs::render(frame, body, app);
        return;
    }

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(24),
            Constraint::Percentage(40),
            Constraint::Percentage(36),
        ])
        .split(body);
    render_tree(frame, cols[0], app);
    match app.playground.pick.clone() {
        Pick::Collection(_) => render_collection(frame, cols[1], app),
        Pick::Request(..) => render_request(frame, cols[1], app),
        Pick::Nothing => {
            let b = panel_block(" Collection ");
            let inner = b.inner(cols[1]);
            frame.render_widget(b, cols[1]);
            app.playground.main_inner = Rect::default();
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from("Pick a collection or a request on the left, or start a new collection."),
                    Line::from(""),
                    Line::styled(
                        "A collection is a saved recipe; its requests are the steps. Send one request, or run the collection in order; extract parts of a result into variables to chain the next request.",
                        dim_style(),
                    ),
                ])
                .wrap(Wrap { trim: false }),
                inner,
            );
        }
    }
    render_response(frame, cols[2], app);
    render_overlay(frame, body, app);
    render_editor(frame, body, app);
}

fn render_tree(frame: &mut Frame, area: Rect, app: &mut App) {
    let pg = &app.playground;
    let b = block(" Collections ", pg.focus == Focus::Tree);
    let inner = b.inner(area);
    frame.render_widget(b, area);
    let recipes = app.recipes.as_deref().unwrap_or_default();
    let rows = tree_rows(recipes);
    let find = |id| recipes.iter().find(|r| r.id == id);
    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let on = i == pg.tree;
            let open = |p: Pick| pg.pick == p;
            match row {
                TreeRow::Globals => item(vec![Span::raw("Globals")], on),
                TreeRow::Runs => item(vec![Span::raw("Runs")], on),
                TreeRow::NewCollection => {
                    item(vec![Span::styled("+ New collection", accent_style())], on)
                }
                TreeRow::FromTemplate => {
                    item(vec![Span::styled("+ From a template…", accent_style())], on)
                }
                TreeRow::Folder(f) => heading(format!("▾ {f}")),
                TreeRow::Collection(id) => {
                    let Some(r) = find(*id) else {
                        return item(Vec::new(), on);
                    };
                    let mut spans = vec![Span::styled(
                        r.name.clone(),
                        if open(Pick::Collection(*id)) {
                            accent_style().add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().add_modifier(Modifier::BOLD)
                        },
                    )];
                    if !r.triggers.is_empty() {
                        spans.push(Span::styled(
                            format!("  ⚡{}", r.triggers.len()),
                            warn_style(),
                        ));
                    }
                    item(spans, on)
                }
                TreeRow::Request(id, sid) => {
                    let label = find(*id)
                        .and_then(|r| r.steps.iter().find(|s| s.id == *sid))
                        .map(tool_label)
                        .unwrap_or_default();
                    let style = if open(Pick::Request(*id, sid.clone())) {
                        accent_style()
                    } else {
                        Style::default()
                    };
                    item(
                        vec![
                            Span::styled(format!("  {sid}"), style),
                            Span::styled(format!("  {label}"), dim_style()),
                        ],
                        on,
                    )
                }
            }
        })
        .collect();
    let pg = &mut app.playground;
    pg.tree_list.select(Some(pg.tree));
    frame.render_stateful_widget(List::new(items), inner, &mut pg.tree_list);
    pg.tree_inner = inner;
}

/// Draw form `lines` (each with the form row it belongs to, `None` for
/// headings) in the main panel with `selected` in view.
fn render_form(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    title: &str,
    lines: Vec<(Option<usize>, ListItem<'static>)>,
    selected: usize,
) {
    let pg = &mut app.playground;
    let b = block(title, pg.focus == Focus::Main);
    let inner = b.inner(area);
    frame.render_widget(b, area);
    let at = lines.iter().position(|(r, _)| *r == Some(selected));
    pg.main_lines = lines.iter().map(|(r, _)| *r).collect();
    let items: Vec<ListItem> = lines.into_iter().map(|(_, i)| i).collect();
    pg.main_list.select(at);
    frame.render_stateful_widget(List::new(items), inner, &mut pg.main_list);
    pg.main_inner = inner;
}

fn button(label: &str, on: bool) -> ListItem<'static> {
    item(
        vec![Span::styled(format!("[ {label} ]"), accent_style())],
        on,
    )
}

fn field(label: &str, value: String, on: bool) -> ListItem<'static> {
    item(
        vec![
            Span::styled(format!("{label:>12}: "), dim_style()),
            Span::raw(value),
        ],
        on,
    )
}

/// `s` cut to `max` characters, ending in … when cut.
fn fit(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    if max > 0 {
        out.push('…');
    }
    out
}

/// A field value on one line (new lines shown as ⏎).
fn one_line(s: &str) -> String {
    s.replace('\n', " ⏎ ")
}

/// A choice row: the options with the current one marked, and `hint`
/// when focused.
fn choice(label: &str, options: &[&str], at: usize, hint: &str, on: bool) -> ListItem<'static> {
    let mut spans = vec![Span::styled(format!("{label:>12}: "), dim_style())];
    for (i, o) in options.iter().enumerate() {
        if i == at {
            spans.push(Span::styled(format!("[{o}]"), accent_style()));
        } else {
            spans.push(Span::styled(format!(" {o} "), dim_style()));
        }
    }
    if on && !hint.is_empty() {
        spans.push(Span::styled(format!("  {hint}"), dim_style()));
    }
    item(spans, on)
}

fn checkbox(checked: bool, label: &str, on: bool) -> ListItem<'static> {
    item(
        vec![
            Span::styled(if checked { "[x] " } else { "[ ] " }, accent_style()),
            Span::raw(label.to_string()),
        ],
        on,
    )
}

/// A builder row's line.
fn builder_line(
    r: &RequestForm,
    row: &RRow,
    resolved: &std::collections::HashMap<String, String>,
    on: bool,
) -> ListItem<'static> {
    // A text field: its value (or `placeholder`), what it resolves to,
    // and `hint` when focused.
    let text = |label: &str, placeholder: &str, hint: &str| {
        let key = r.text_key(row).unwrap_or_default();
        let value = r.values.get(key).cloned().unwrap_or_default();
        let mut spans = vec![Span::styled(format!("{label:>12}: "), dim_style())];
        if value.is_empty() {
            spans.push(Span::styled(placeholder.to_string(), dim_style()));
        } else {
            spans.push(Span::raw(one_line(&value)));
        }
        if let Some(res) = resolved.get(key).filter(|v| !v.is_empty() && **v != value) {
            spans.push(Span::styled(
                format!("  → {}", one_line(res)),
                muted_style(),
            ));
        }
        if on && !hint.is_empty() {
            spans.push(Span::styled(format!("  {hint}"), dim_style()));
        }
        item(spans, on)
    };
    let provider = &PROVIDERS[r.provider()];
    match row {
        RRow::Plain => checkbox(r.plain, "Plain fields (instead of the builder)", on),
        RRow::Http(h) => match *h {
            HttpRow::Method => {
                let method = r.values.get("method").map_or("GET", String::as_str);
                let at = HTTP_METHODS
                    .iter()
                    .position(|m| m.eq_ignore_ascii_case(method))
                    .unwrap_or(0);
                item(
                    vec![
                        Span::styled(format!("{:>12}: ", "Method"), dim_style()),
                        Span::styled(HTTP_METHODS[at], accent_style()),
                        Span::styled(if on { "  ←/→ change" } else { "" }, dim_style()),
                    ],
                    on,
                )
            }
            HttpRow::Url => text("URL", "https://api.example.com/v1/items/{{id}}", ""),
            HttpRow::Query(i) | HttpRow::Header(i) => {
                let key = if matches!(h, HttpRow::Query(_)) {
                    "query"
                } else {
                    "headers"
                };
                let kv = r
                    .kv(key)
                    .get(i)
                    .map(|kv| kv_text(key, kv))
                    .unwrap_or_default();
                item(vec![Span::raw(kv)], on)
            }
            HttpRow::AddQuery => button("+ Query param", on),
            HttpRow::AddHeader => button("+ Header", on),
            HttpRow::Body => {
                let mode = BodyMode::of(&r.values);
                let labels: Vec<&str> = BodyMode::ALL.iter().map(|m| m.label()).collect();
                let at = BodyMode::ALL.iter().position(|m| *m == mode).unwrap_or(0);
                choice("Body", &labels, at, "←/→ change", on)
            }
            HttpRow::BodyText => {
                let json = BodyMode::of(&r.values) == BodyMode::Json;
                let mut line = text(
                    if json { "JSON" } else { "Text" },
                    if json {
                        r#"{"name": "{{name}}"}"#
                    } else {
                        "text body"
                    },
                    "Alt+Enter: new line",
                );
                if let Some(e) = json_body_error(&r.values) {
                    line = ListItem::new(Line::from(vec![
                        Span::styled(if on { "▸ " } else { "  " }, accent_style()),
                        Span::styled(format!("{:>12}: ", "JSON"), dim_style()),
                        Span::raw(one_line(&r.values["json"])),
                        Span::styled(format!("  not JSON yet: {e}"), warn_style()),
                    ]));
                }
                line
            }
            HttpRow::Check => checkbox(
                r.values.get("check").is_some_and(|c| c == "true"),
                "Fail on a non-2xx status (stops a collection run)",
                on,
            ),
        },
        RRow::Llm(l) => match l {
            LlmRow::Provider => {
                let names: Vec<&str> = PROVIDERS.iter().map(|p| p.name).collect();
                choice("Provider", &names, r.provider(), provider.hint, on)
            }
            LlmRow::BaseUrl => text("Base URL", "https://my-server/v1", ""),
            LlmRow::KeyFrom => {
                let mode = r.key_mode();
                let labels: Vec<&str> = KeyMode::ALL.iter().map(|m| m.label()).collect();
                let at = KeyMode::ALL.iter().position(|m| *m == mode).unwrap_or(0);
                choice("Key", &labels, at, "", on)
            }
            LlmRow::Key => match r.key_mode() {
                KeyMode::Env => text(
                    "Env var",
                    provider.key_env.unwrap_or("LLM_API_KEY"),
                    "read from herdr-mcp's environment when the request runs",
                ),
                _ => text(
                    "Key",
                    "",
                    "keep it in a variable (e.g. {{llm_api_key}}): a pasted key is saved in the collection",
                ),
            },
            LlmRow::Model => text(
                "Model",
                if provider.name == "OpenRouter" {
                    "openai/gpt-4o-mini"
                } else {
                    "model id"
                },
                "",
            ),
            LlmRow::Models => button(&format!("Load models from {}", provider.name), on),
            LlmRow::System => text("System", "(optional)", "Alt+Enter: new line"),
            LlmRow::Prompt => text(
                "Prompt",
                "Summarise this: {{read.output}}",
                "Alt+Enter: new line; later requests read the reply as {{this-request.text}}",
            ),
            LlmRow::Temperature => text("Temperature", "model default", ""),
            LlmRow::MaxTokens => text("Max tokens", "model default", ""),
        },
        _ => item(Vec::new(), on),
    }
}

fn render_collection(frame: &mut Frame, area: Rect, app: &mut App) {
    let requests = request_count(app);
    let Some(c) = app.playground.collection.as_ref() else {
        return;
    };
    let recipe = app
        .recipes
        .as_ref()
        .and_then(|rs| rs.iter().find(|r| r.id == c.id));
    let steps = recipe.map(|r| r.steps.clone()).unwrap_or_default();
    let busy = app.job.is_some();
    let rows = c.rows(requests);
    let mut lines: Vec<(Option<usize>, ListItem<'static>)> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let on = i == c.row;
        match row {
            CRow::Request(0) | CRow::AddRequest
                if !matches!(rows.get(i.wrapping_sub(1)), Some(CRow::Request(_))) =>
            {
                lines.push((None, ListItem::new("")));
                lines.push((None, heading(format!("Requests · {}", steps.len()))));
            }
            CRow::Var(0) => {
                lines.push((None, ListItem::new("")));
                lines.push((None, heading("Collection variables".into())));
                lines.push((
                    None,
                    ListItem::new(Line::styled(
                        "  seen over globals; extract rules with scope collection write them",
                        dim_style(),
                    )),
                ));
            }
            CRow::AddVar if c.vars.is_empty() => {
                lines.push((None, ListItem::new("")));
                lines.push((None, heading("Collection variables".into())));
            }
            CRow::Trigger(0) => {
                lines.push((None, ListItem::new("")));
                lines.push((None, heading("Triggers".into())));
                lines.push((
                    None,
                    ListItem::new(Line::styled(
                        "  run this collection on a herdr event: `event key=value, …` (see Ctrl+E)",
                        dim_style(),
                    )),
                ));
            }
            CRow::AddTrigger if c.triggers.is_empty() => {
                lines.push((None, ListItem::new("")));
                lines.push((None, heading("Triggers".into())));
            }
            CRow::Gate(0) | CRow::AddGate
                if !matches!(rows.get(i.wrapping_sub(1)), Some(CRow::Gate(_))) =>
            {
                lines.push((None, ListItem::new("")));
                lines.push((None, heading("Gate · after the last request".into())));
                lines.push((
                    None,
                    ListItem::new(Line::styled(
                        "  first rule that holds: end, restart (loop) or fail; `path op value → action`",
                        dim_style(),
                    )),
                ));
            }
            CRow::Trail(TrailField::KeepRuns) => {
                lines.push((None, ListItem::new("")));
                lines.push((None, heading("Run history".into())));
                lines.push((
                    None,
                    ListItem::new(Line::styled(
                        "  blank uses the global setting (Settings › runs); 0 means no limit",
                        dim_style(),
                    )),
                ));
            }
            CRow::Save => lines.push((None, ListItem::new(""))),
            _ => {}
        }
        let line = match row {
            CRow::Name => field("Collection", c.name.clone(), on),
            CRow::Folder => field("Folder", c.folder.clone(), on),
            CRow::Request(n) => {
                let s = &steps[*n];
                let mut spans = vec![
                    Span::styled(s.id.clone(), accent_style()),
                    Span::styled(format!("  {}", tool_label(s)), dim_style()),
                ];
                if !s.extract.is_empty() {
                    spans.push(Span::styled(
                        format!("  → {} var(s)", s.extract.len()),
                        muted_style(),
                    ));
                }
                item(spans, on)
            }
            CRow::AddRequest => button("+ Request", on),
            CRow::AddHttp => button("+ HTTP", on),
            CRow::AddLlm => button("+ LLM", on),
            CRow::RunAll => button(if busy { "Running…" } else { "Run collection" }, on),
            CRow::Runs => button("Runs ›", on),
            CRow::Gate(n) => item(
                vec![
                    Span::styled(format!("{}. ", n + 1), dim_style()),
                    Span::raw(rule_text(&c.gate[*n])),
                ],
                on,
            ),
            CRow::AddGate => button("+ Gate rule", on),
            CRow::MaxSteps => field("Max requests", c.max_steps.to_string(), on),
            CRow::Trail(f) => {
                let value = match f.get(&c.trail) {
                    Some(n) => n.to_string(),
                    None => match app.knobs.number(f.setting()) {
                        0 if *f == TrailField::SnapshotLines => "global: none".into(),
                        0 => "global: keep all".into(),
                        n => format!("global: {n}"),
                    },
                };
                field(f.label(), value, on)
            }
            CRow::Var(n) => {
                let (k, v) = &c.vars[*n];
                item(
                    vec![
                        Span::styled(k.clone(), accent_style()),
                        Span::styled(" = ", dim_style()),
                        Span::raw(v.clone()),
                    ],
                    on,
                )
            }
            CRow::AddVar => button("+ Variable", on),
            CRow::Trigger(n) => item(vec![Span::raw(trigger_text(&c.triggers[*n]))], on),
            CRow::AddTrigger => button("+ Trigger", on),
            CRow::Save => button(
                if c.changed() {
                    "Save collection *"
                } else {
                    "Save collection"
                },
                on,
            ),
            CRow::Delete => button(
                if c.confirm_delete {
                    "Delete collection — Enter again to confirm"
                } else {
                    "Delete collection"
                },
                on,
            ),
        };
        lines.push((Some(i), line));
    }
    lines.push((None, ListItem::new("")));
    lines.push((
        None,
        ListItem::new(Line::styled(
            "  Ctrl+↑/↓ move a request  Ctrl+D remove  Esc tree",
            dim_style(),
        )),
    ));
    let row = c.row;
    render_form(frame, area, app, " Collection ", lines, row);
}

fn render_request(frame: &mut Frame, area: Rect, app: &mut App) {
    let pg = &app.playground;
    let Some(r) = pg.request.as_ref() else {
        return;
    };
    let collection = app
        .recipes
        .as_ref()
        .and_then(|rs| rs.iter().find(|x| x.id == r.rid))
        .map(|x| x.name.clone())
        .unwrap_or_default();
    let (props, required) = r.schema(&pg.tools, &pg.methods);
    let resolved = r.resolved_values();
    let rows = r.rows(&pg.tools, &pg.methods);
    let busy = app.job.is_some();
    let mut lines: Vec<(Option<usize>, ListItem<'static>)> = vec![(
        None,
        ListItem::new(Line::from(vec![
            Span::styled(collection, muted_style()),
            Span::styled(format!(" › {}", r.saved_id), dim_style()),
        ])),
    )];
    for (i, row) in rows.iter().enumerate() {
        let on = i == r.row;
        match row {
            RRow::Param(_) if !matches!(rows.get(i - 1), Some(RRow::Param(_))) => {
                lines.push((None, ListItem::new("")));
                lines.push((None, heading("Params".into())));
            }
            RRow::Http(HttpRow::Method)
            | RRow::Http(HttpRow::Query(0))
            | RRow::Http(HttpRow::Header(0))
            | RRow::Http(HttpRow::Body)
            | RRow::Llm(LlmRow::Provider)
            | RRow::Llm(LlmRow::Model)
            | RRow::Llm(LlmRow::System) => {
                let title = match row {
                    RRow::Http(HttpRow::Method) => "Endpoint",
                    RRow::Http(HttpRow::Query(_)) => "Query",
                    RRow::Http(HttpRow::Header(_)) => "Headers",
                    RRow::Http(HttpRow::Body) => "Body",
                    RRow::Llm(LlmRow::Provider) => "Provider and key",
                    RRow::Llm(LlmRow::Model) => "Model",
                    _ => "Prompt",
                };
                lines.push((None, ListItem::new("")));
                lines.push((None, heading(title.into())));
            }
            RRow::Http(HttpRow::AddQuery) if r.kv("query").is_empty() => {
                lines.push((None, ListItem::new("")));
                lines.push((None, heading("Query".into())));
            }
            RRow::Http(HttpRow::AddHeader) if r.kv("headers").is_empty() => {
                lines.push((None, ListItem::new("")));
                lines.push((None, heading("Headers".into())));
            }
            RRow::Extract(0) => {
                lines.push((None, ListItem::new("")));
                lines.push((None, heading("Extract → variables".into())));
                lines.push((
                    None,
                    ListItem::new(Line::styled(
                        "  path -> var; Space toggles global/collection; or Enter on a result path",
                        dim_style(),
                    )),
                ));
            }
            RRow::AddExtract if r.extract.is_empty() => {
                lines.push((None, ListItem::new("")));
                lines.push((None, heading("Extract → variables".into())));
            }
            RRow::Send => {
                if !r.unresolved.is_empty() {
                    lines.push((None, ListItem::new("")));
                    lines.push((None, ListItem::new(Line::styled(
                        format!(
                            "  not resolved before sending: {} (a request's result only exists during a run; extract it to chain sends)",
                            r.unresolved.iter().map(|u| format!("{{{{{u}}}}}")).collect::<Vec<_>>().join(" ")
                        ),
                        warn_style(),
                    ))));
                }
                lines.push((None, ListItem::new("")));
            }
            _ => {}
        }
        let line = match row {
            RRow::Id => field("Request", r.id.clone(), on),
            RRow::Tool => {
                // Borders, the row marker, the label and the gap take the rest.
                let room = (area.width as usize).saturating_sub(20 + r.tool.chars().count());
                let desc = pg
                    .tools
                    .iter()
                    .find(|t| t.name == r.tool)
                    .map(|t| fit(t.description.lines().next().unwrap_or(""), room))
                    .unwrap_or_default();
                item(
                    vec![
                        Span::styled(format!("{:>12}: ", "Tool"), dim_style()),
                        Span::styled(r.tool.clone(), accent_style()),
                        Span::styled(format!("  {desc}"), dim_style()),
                    ],
                    on,
                )
            }
            RRow::Description => field("Description", r.description.clone(), on),
            RRow::Method => item(
                vec![
                    Span::styled(format!("{:>12}: ", "herdr method"), dim_style()),
                    Span::styled(r.method.clone(), accent_style()),
                ],
                on,
            ),
            RRow::Param(name) => {
                let prop = props.get(name);
                let value = r.values.get(name).cloned().unwrap_or_default();
                let kind = prop.map(herdr_mcp_api::schema_type).unwrap_or("string");
                let shown = match kind {
                    "boolean" if value.is_empty() => "[ ]".to_string(),
                    "boolean" => format!("[{}]", if value == "true" { "x" } else { " " }),
                    _ if value.is_empty() => String::new(),
                    _ => value.clone(),
                };
                let mut spans = vec![
                    Span::styled(
                        format!("{name}{}", if required.contains(name) { "*" } else { "" }),
                        if required.contains(name) {
                            accent_style()
                        } else {
                            Style::default()
                        },
                    ),
                    Span::styled(": ", dim_style()),
                    Span::raw(shown),
                ];
                if let Some(res) = resolved.get(name).filter(|v| **v != value) {
                    spans.push(Span::styled(format!("  → {res}"), muted_style()));
                }
                if on && let Some(p) = prop {
                    let mut hint = p
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(e) = p.get("enum").and_then(|e| e.as_array()) {
                        let opts: Vec<String> = e.iter().map(display_value).collect();
                        hint = format!("{hint} ←/→: {}", opts.join("|")).trim().to_string();
                    } else if value.is_empty()
                        && let Some(d) = p.get("default")
                    {
                        hint = format!("{hint} (default {d})").trim().to_string();
                    }
                    if !hint.is_empty() {
                        spans.push(Span::styled(format!("  {hint}"), dim_style()));
                    }
                }
                item(spans, on)
            }
            RRow::Extract(n) => {
                let x = &r.extract[*n];
                let scope = match x.scope {
                    VarScope::Global => "global",
                    VarScope::Collection => "collection",
                };
                item(
                    vec![
                        Span::raw(extract_text(x)),
                        Span::styled(format!("  ({scope})"), dim_style()),
                    ],
                    on,
                )
            }
            RRow::Plain | RRow::Http(_) | RRow::Llm(_) => builder_line(r, row, &resolved, on),
            RRow::AddExtract => button("+ Extract", on),
            RRow::Send => button(if busy { "Sending…" } else { "Send" }, on),
            RRow::Save => button(
                if r.unsaved(&pg.tools, &pg.methods) {
                    "Save *"
                } else {
                    "Save"
                },
                on,
            ),
            RRow::Delete => button(
                if r.confirm_delete {
                    "Delete — Enter again to confirm"
                } else {
                    "Delete"
                },
                on,
            ),
        };
        lines.push((Some(i), line));
    }
    lines.push((None, ListItem::new("")));
    lines.push((
        None,
        ListItem::new(Line::styled(
            "  type to edit  Space bool  ←/→ choice  Ctrl+D clear/remove  Esc tree",
            dim_style(),
        )),
    ));
    let row = r.row;
    render_form(frame, area, app, " Request ", lines, row);
}

fn render_response(frame: &mut Frame, area: Rect, app: &mut App) {
    let pg = &mut app.playground;
    let paths = pg
        .request
        .as_ref()
        .filter(|_| matches!(pg.pick, Pick::Request(..)))
        .map(|r| (r.paths.clone(), r.path_index))
        .filter(|(p, _)| !p.is_empty());
    let (top, bottom) = match &paths {
        Some(_) => {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                .split(area);
            (rows[0], Some(rows[1]))
        }
        None => (area, None),
    };
    let b = block(" Response ", pg.focus == Focus::Response);
    let inner = b.inner(top);
    frame.render_widget(b, top);
    pg.result_inner = inner;
    if pg.result_area.lines().iter().all(|l| l.is_empty()) {
        let hint = match pg.pick {
            Pick::Collection(_) => "Run the collection (Ctrl+G) to see each request's result.",
            Pick::Request(..) => "Send the request (Ctrl+G) to see its response.",
            Pick::Nothing => "",
        };
        frame.render_widget(
            Paragraph::new(Line::styled(hint, dim_style())).wrap(Wrap { trim: false }),
            inner,
        );
    } else {
        frame.render_widget(&pg.result_area, inner);
    }
    let (Some(area), Some((paths, index))) = (bottom, paths) else {
        pg.paths_inner = Rect::default();
        return;
    };
    let b = block(" Result paths · Enter extracts ", pg.focus == Focus::Paths);
    let inner = b.inner(area);
    frame.render_widget(b, area);
    let items: Vec<ListItem> = paths
        .iter()
        .enumerate()
        .map(|(i, (path, value))| {
            item(
                vec![
                    Span::styled(path.clone(), accent_style()),
                    Span::styled(format!("  {}", display_value(value)), dim_style()),
                ],
                i == index,
            )
        })
        .collect();
    pg.paths_list.select(Some(index));
    frame.render_stateful_widget(List::new(items), inner, &mut pg.paths_list);
    pg.paths_inner = inner;
}

/// A box centred in `area`, `w`% wide and `h` rows high, cleared.
fn modal(frame: &mut Frame, area: Rect, w: u16, h: u16, title: &str) -> Rect {
    let w = (area.width * w / 100).max(40).min(area.width);
    let h = h.min(area.height);
    let rect = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    );
    frame.render_widget(Clear, rect);
    let b = Block::default()
        .borders(Borders::ALL)
        .title(title.to_string())
        .border_style(accent_style());
    let inner = b.inner(rect);
    frame.render_widget(b, rect);
    inner
}

/// A filterable list modal: the search line, then the items.
fn picker(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    search: &str,
    items: Vec<ListItem<'static>>,
    index: usize,
) {
    let inner = modal(frame, area, 60, area.height * 7 / 10, title);
    let search_line = if search.is_empty() {
        Line::styled("type to filter…   Enter pick  Esc cancel", dim_style())
    } else {
        Line::from(vec![
            Span::raw(search.to_string()),
            Span::styled("▌", accent_style()),
        ])
    };
    frame.render_widget(
        Paragraph::new(search_line),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let mut state = ListState::default();
    state.select(Some(index));
    frame.render_stateful_widget(
        List::new(items),
        Rect::new(
            inner.x,
            inner.y + 2,
            inner.width,
            inner.height.saturating_sub(2),
        ),
        &mut state,
    );
}

fn render_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let pg = &app.playground;
    let Some(overlay) = &pg.overlay else {
        return;
    };
    match overlay {
        Overlay::Tools { search, index } => {
            let items = tool_choices(&pg.tools, search)
                .into_iter()
                .enumerate()
                .map(|(i, t)| {
                    item(
                        vec![
                            Span::styled(format!("{:<10} ", t.category), dim_style()),
                            Span::styled(
                                t.name.clone(),
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("  {}", t.description.lines().next().unwrap_or("")),
                                dim_style(),
                            ),
                        ],
                        i == *index,
                    )
                })
                .collect();
            picker(frame, area, " Tool ", search, items, *index);
        }
        Overlay::Methods { search, index } => {
            let items = method_choices(&pg.methods, search)
                .into_iter()
                .enumerate()
                .map(|(i, m)| item(vec![Span::raw(m.to_string())], i == *index))
                .collect();
            picker(frame, area, " herdr method ", search, items, *index);
        }
        Overlay::Models { search, index } => {
            let models = pg
                .request
                .as_ref()
                .map(|r| r.models.as_slice())
                .unwrap_or_default();
            let items = model_choices(models, search)
                .into_iter()
                .enumerate()
                .map(|(i, (id, ctx))| {
                    item(
                        vec![Span::raw(id), Span::styled(format!("  {ctx}"), dim_style())],
                        i == *index,
                    )
                })
                .collect();
            picker(frame, area, " Model ", search, items, *index);
        }
        Overlay::Templates { index } => {
            let templates = app.templates.as_deref().unwrap_or_default();
            let items = templates
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    item(
                        vec![
                            Span::styled(
                                t.name.clone(),
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("  [{}]  {}", t.category, t.description),
                                dim_style(),
                            ),
                        ],
                        i == *index,
                    )
                })
                .collect();
            picker(frame, area, " New from a template ", "", items, *index);
        }
        Overlay::Template {
            template,
            values,
            row,
            error,
        } => {
            let vars = template.variables.len();
            let inner = modal(
                frame,
                area,
                60,
                (vars as u16) * 2 + 8,
                &format!(" {} ", template.name),
            );
            let mut lines = vec![
                Line::styled(template.description.clone(), dim_style()),
                Line::from(""),
            ];
            for (i, v) in template.variables.iter().enumerate() {
                let on = i == *row;
                let line = Line::from(vec![
                    Span::styled(if on { "▸ " } else { "  " }, accent_style()),
                    Span::styled(
                        format!("{}{}", v.name, if v.required { "*" } else { "" }),
                        accent_style(),
                    ),
                    Span::styled(": ", dim_style()),
                    Span::raw(values[i].clone()),
                ]);
                lines.push(if on {
                    line.style(selected_style())
                } else {
                    line
                });
                lines.push(Line::styled(format!("    {}", v.description), dim_style()));
            }
            for (i, label) in [(vars, "[ Create collection ]"), (vars + 1, "[ Cancel ]")] {
                let on = i == *row;
                let line = Line::from(vec![
                    Span::styled(if on { "▸ " } else { "  " }, accent_style()),
                    Span::styled(label, accent_style()),
                ]);
                lines.push(if on {
                    line.style(selected_style())
                } else {
                    line
                });
            }
            if let Some(e) = error {
                lines.push(Line::styled(format!("⚠ {e}"), warn_style()));
            }
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
    }
}

/// The text box of a field being typed into, over the panels.
fn render_editor(frame: &mut Frame, area: Rect, app: &App) {
    let pg = &app.playground;
    let Some(editing) = &pg.editing else {
        return;
    };
    let (title, hint) = match &editing.target {
        Target::CollectionName => ("Collection name".to_string(), String::new()),
        Target::Folder => ("Folder".to_string(), "groups collections in the tree; empty for none".into()),
        Target::Var(_) => ("Collection variable".to_string(), "name = value (JSON values are stored typed)".into()),
        Target::Trigger(_) => (
            "Trigger".to_string(),
            format!(
                "event key=value, …   events: {}",
                pg.event_names.join(" ")
            ),
        ),
        Target::RequestId => (
            "Request name".to_string(),
            "letters, digits, _ and -; other requests read its result as {{ name.path }}".into(),
        ),
        Target::Description => ("Description".to_string(), String::new()),
        Target::Param(name) => (
            format!("Param {name}"),
            "{{ var }} placeholders resolve on the server; Alt+Enter new line".into(),
        ),
        Target::Kv(key, _) if *key == "headers" => (
            "Header".to_string(),
            "Name: value ({{ var }} resolves); empty removes it".into(),
        ),
        Target::Kv(..) => (
            "Query param".to_string(),
            "name=value ({{ var }} resolves); empty removes it".into(),
        ),
        Target::Extract(_) => (
            "Extract rule".to_string(),
            "path -> var: the path is followed into the result (e.g. panes[0].pane_id); empty saves its text".into(),
        ),
        Target::TemplateVar(_) => ("Template variable".to_string(), String::new()),
        Target::Gate(_) => (
            "Gate rule".to_string(),
            "path op value [and|or …] → end | restart 500ms | fail message; ops: eq ne lt gt contains exists missing; `always → end` always matches".into(),
        ),
        Target::MaxSteps => (
            "Max requests per run".to_string(),
            "every pass of a restart loop counts; a run that would go over fails".into(),
        ),
        Target::Trail(f) => (
            f.label().to_string(),
            "a whole number; blank uses the global setting; 0 means no limit".into(),
        ),
    };
    let height = editing.area.lines().len() as u16 + 5;
    let inner = modal(frame, area, 70, height.max(6), &format!(" {title} "));
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!("Enter done · Esc cancel  {hint}"),
            dim_style(),
        ))
        .wrap(Wrap { trim: true }),
        Rect::new(inner.x, inner.y, inner.width, 2.min(inner.height)),
    );
    frame.render_widget(
        &editing.area,
        Rect::new(
            inner.x,
            inner.y + 2,
            inner.width,
            inner.height.saturating_sub(2),
        ),
    );
}

fn render_events(frame: &mut Frame, area: Rect, app: &mut App) {
    let ev = &mut app.playground.events;
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);
    let b = focused_panel_block(" herdr events ");
    let inner = b.inner(cols[0]);
    frame.render_widget(b, cols[0]);
    let filter = if ev.filter.is_empty() {
        Line::styled("type to filter by event name", dim_style())
    } else {
        Line::from(vec![
            Span::styled("filter: ", dim_style()),
            Span::raw(ev.filter.clone()),
            Span::styled("▌", accent_style()),
        ])
    };
    frame.render_widget(
        Paragraph::new(filter),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let list_area = Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height.saturating_sub(1),
    );
    let shown = ev.shown();
    let picked = shown.get(ev.index).map(|e| (*e).clone());
    if shown.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "Waiting for herdr events… (create a pane, switch tabs, or run an agent)",
                dim_style(),
            ))
            .wrap(Wrap { trim: false }),
            list_area,
        );
    }
    let items: Vec<ListItem> = shown
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let mut spans = vec![Span::styled(e.event.clone(), accent_style())];
            if let Some(ws) = e.workspace_id() {
                spans.push(Span::styled(format!("  {ws}"), dim_style()));
            }
            item(spans, i == ev.index)
        })
        .collect();
    ev.list.select(Some(ev.index));
    frame.render_stateful_widget(List::new(items), list_area, &mut ev.list);
    ev.list_inner = list_area;

    let b = panel_block(" Event data ");
    let inner = b.inner(cols[1]);
    frame.render_widget(b, cols[1]);
    let mut lines = vec![Line::styled(
        "A trigger fires on the event name, optionally filtered by values in its data (agent_status=done); the run reads the data as {{herdr.…}}.",
        dim_style(),
    )];
    if let Some(e) = picked {
        lines.push(Line::from(""));
        lines.push(Line::styled(
            e.event.clone(),
            accent_style().add_modifier(Modifier::BOLD),
        ));
        lines.extend(
            serde_json::to_string_pretty(&e.data)
                .unwrap_or_default()
                .lines()
                .map(|l| Line::from(l.to_string())),
        );
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

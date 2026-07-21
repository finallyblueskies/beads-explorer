use crate::app::{AddIssueFlow, AddIssueStep, App, Screen, ISSUE_TYPES, PRIORITIES};
use crate::model::Issue;
use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate};
use std::io::{self, Write};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const SEPARATOR: &str = " │ ";
const HIGHLIGHT_BACKGROUND: Color = Color::AnsiValue(238);
const MODAL_BACKGROUND: Color = Color::AnsiValue(235);

#[derive(Clone, Copy)]
enum LineStyle {
    Plain,
    Bold,
    Dim,
    Header,
    Selected,
    Dependency,
}

struct DetailLine {
    text: String,
    style: LineStyle,
    dependency: Option<usize>,
}

/// Issue behind a table row, for cell colors; `None` renders the header.
struct RowState<'a> {
    selected: bool,
    issue: &'a Issue,
}

pub fn draw(app: &mut App, out: &mut impl Write, width: u16, height: u16) -> io::Result<()> {
    queue!(out, BeginSynchronizedUpdate, MoveTo(0, 0))?;
    match app.screen() {
        Screen::Tree => draw_tree(app, out, width, height)?,
        Screen::Detail => draw_detail(app, out, width, height)?,
    }
    if let Some(flow) = app.add_issue_flow() {
        draw_add_issue(out, width, height, flow, app.status_message())?;
        if flow.is_confirming_cancel() {
            draw_add_issue_cancel_confirmation(out, width, height)?;
        }
    }
    if let Some(issue_id) = app.closing_issue_id() {
        draw_close_confirmation(out, width, height, issue_id)?;
    }
    queue!(out, EndSynchronizedUpdate)?;
    out.flush()
}

fn draw_tree(app: &mut App, out: &mut impl Write, width: u16, height: u16) -> io::Result<()> {
    let width = width as usize;
    if height == 0 || width == 0 {
        return Ok(());
    }

    let id_width = app
        .rows
        .iter()
        .map(|row| row.issue_id.width())
        .max()
        .unwrap_or(2);
    let widths = table_widths(width, id_width);
    write_table_row(
        out,
        0,
        &["ID", "PRIORITY", "STATUS", "TYPE", "NAME"],
        widths,
        None,
    )?;

    let list_height = height.saturating_sub(2) as usize;
    app.viewport = list_height.max(1);
    let mut content_end: u16 = 1;
    if app.rows.is_empty() {
        let message = if let Some(query) = app.search_query() {
            format!("No issue IDs match /{query}")
        } else {
            "No issues found. Create one with `bd create`.".to_string()
        };
        queue!(
            out,
            MoveTo(0, 1),
            Clear(ClearType::CurrentLine),
            SetAttribute(Attribute::Dim),
            Print(truncate(&message, width)),
            SetAttribute(Attribute::Reset)
        )?;
        content_end = 2;
    } else if list_height > 0 {
        if app.scroll + list_height > app.rows.len() {
            app.scroll = app.rows.len().saturating_sub(list_height);
        }
        if app.cursor < app.scroll {
            app.scroll = app.cursor;
        }
        if app.cursor >= app.scroll + list_height {
            app.scroll = app.cursor + 1 - list_height;
        }

        let visible = list_height.min(app.rows.len().saturating_sub(app.scroll));
        for line in 0..visible {
            let position = app.scroll + line;
            let row = &app.rows[position];
            let Some(issue) = app.graph.issue(&row.issue_id) else {
                queue!(
                    out,
                    MoveTo(0, line as u16 + 1),
                    Clear(ClearType::CurrentLine)
                )?;
                continue;
            };
            let marker = if app.search_query().is_some() {
                "  "
            } else if row.cycle {
                "↻ "
            } else if app.row_has_children(row) {
                if app.row_is_expanded(row) {
                    "▾ "
                } else {
                    "▸ "
                }
            } else {
                "  "
            };
            let name = format!("{}{}{}", row.prefix, marker, issue.title);
            let priority = format!("P{}", issue.priority);
            let issue_type = short_type(&issue.issue_type);
            write_table_row(
                out,
                line as u16 + 1,
                &[&issue.id, &priority, &issue.status, &issue_type, &name],
                widths,
                Some(RowState {
                    selected: position == app.cursor,
                    issue,
                }),
            )?;
        }
        content_end = visible as u16 + 1;
    }
    queue!(
        out,
        MoveTo(0, content_end),
        Clear(ClearType::FromCursorDown)
    )?;

    if let Some(status) = app.status_message() {
        return draw_status_line(out, width, height, status);
    }

    let footer = if let Some(query) = app.search_query() {
        format!(
            "/{query}  · {} match{} · ↑/↓ select · Enter open · Esc cancel",
            app.rows.len(),
            if app.rows.len() == 1 { "" } else { "es" }
        )
    } else {
        format!(
            "{} issue{} · + add child · / go to · j/k move · h/l fold · Enter open · e edit · x close · q quit",
            app.graph.len(),
            if app.graph.len() == 1 { "" } else { "s" }
        )
    };
    queue!(
        out,
        MoveTo(0, height - 1),
        Clear(ClearType::CurrentLine),
        SetAttribute(if app.search_query().is_some() {
            Attribute::Bold
        } else {
            Attribute::Dim
        }),
        Print(truncate(&footer, width)),
        SetAttribute(Attribute::Reset)
    )?;
    Ok(())
}

fn draw_detail(app: &mut App, out: &mut impl Write, width: u16, height: u16) -> io::Result<()> {
    let width = width as usize;
    if height == 0 || width == 0 {
        return Ok(());
    }
    let Some(issue) = app.current_detail_issue().cloned() else {
        queue!(out, MoveTo(0, 0), Clear(ClearType::FromCursorDown))?;
        return Ok(());
    };
    let selected_dependency = app
        .detail_frame()
        .map(|frame| frame.dependency_cursor)
        .unwrap_or(0);
    let lines = detail_lines(&issue, width, selected_dependency);
    let visible = height.saturating_sub(1) as usize;
    let selected_line = lines
        .iter()
        .position(|line| line.dependency == Some(selected_dependency));
    let max_scroll = lines.len().saturating_sub(visible);
    let mut scroll = app.detail_frame().map(|frame| frame.scroll).unwrap_or(0);
    scroll = scroll.min(max_scroll);
    if let Some(selected) = selected_line {
        if selected < scroll {
            scroll = selected;
        } else if visible > 0 && selected >= scroll + visible {
            scroll = selected + 1 - visible;
        }
    }
    if let Some(frame) = app.detail_frame_mut() {
        frame.scroll = scroll;
    }

    let mut content_end: u16 = 0;
    for (screen_line, line) in lines.iter().skip(scroll).take(visible).enumerate() {
        queue!(
            out,
            MoveTo(0, screen_line as u16),
            Clear(ClearType::UntilNewLine)
        )?;
        apply_line_style(out, line.style)?;
        queue!(out, Print(truncate(&line.text, width)))?;
        if matches!(line.style, LineStyle::Selected) {
            let used = line.text.width().min(width);
            if used < width {
                queue!(out, Print(" ".repeat(width - used)))?;
            }
        }
        queue!(out, ResetColor, SetAttribute(Attribute::Reset))?;
        content_end = screen_line as u16 + 1;
    }
    queue!(
        out,
        MoveTo(0, content_end),
        Clear(ClearType::FromCursorDown)
    )?;

    if let Some(status) = app.status_message() {
        return draw_status_line(out, width, height, status);
    }

    let footer = if app.can_close_current_issue() {
        "+ add child · j/k dependency · Enter open · e description · et title · x close · Backspace back · Esc tree · q quit"
            .to_string()
    } else {
        "+ add child · j/k dependency · Enter open · e description · et title · Backspace back · Esc tree · q quit"
            .to_string()
    };
    queue!(
        out,
        MoveTo(0, height - 1),
        Clear(ClearType::CurrentLine),
        SetAttribute(Attribute::Dim),
        Print(truncate(&footer, width)),
        SetAttribute(Attribute::Reset),
    )?;
    Ok(())
}

fn draw_status_line(
    out: &mut impl Write,
    width: usize,
    height: u16,
    status: &str,
) -> io::Result<()> {
    queue!(
        out,
        MoveTo(0, height - 1),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(Color::Yellow),
        SetAttribute(Attribute::Bold),
        Print(truncate(status, width)),
        ResetColor,
        SetAttribute(Attribute::Reset)
    )
}

fn draw_close_confirmation(
    out: &mut impl Write,
    width: u16,
    height: u16,
    issue_id: &str,
) -> io::Result<()> {
    draw_floating_modal(
        out,
        width,
        height,
        " Confirm close ",
        &[
            format!("Close issue {issue_id}?"),
            String::new(),
            "[y] Yes    [n/Esc] No".to_string(),
        ],
    )
}

fn draw_add_issue(
    out: &mut impl Write,
    width: u16,
    height: u16,
    flow: &AddIssueFlow,
    status: Option<&str>,
) -> io::Result<()> {
    let mut lines = vec![format!("Parent: {}", flow.parent_id), String::new()];
    match flow.step {
        AddIssueStep::Title => {
            lines.push("Title".to_string());
            lines.push(format!("> {}▏", flow.title));
            lines.push(String::new());
            lines.push("Type a title · Enter continue · e $EDITOR · Esc cancel".to_string());
        }
        AddIssueStep::Description => {
            lines.push("Description".to_string());
            let preview = flow.description.replace('\n', " ↵ ");
            lines.push(format!("> {}▏", preview));
            lines.push(String::new());
            lines.push("Type a description · Enter continue · e $EDITOR · Esc cancel".to_string());
        }
        AddIssueStep::IssueType => {
            lines.push("Select issue type".to_string());
            lines.extend(ISSUE_TYPES.iter().enumerate().map(|(index, issue_type)| {
                format!(
                    "{} {}",
                    if index == flow.issue_type_index {
                        "›"
                    } else {
                        " "
                    },
                    issue_type
                )
            }));
            lines.push(
                "j/k or ↑/↓ select · Enter continue · Backspace previous · Esc cancel".to_string(),
            );
        }
        AddIssueStep::Priority => {
            lines.push("Select priority (P1 is the default)".to_string());
            lines.extend(PRIORITIES.iter().enumerate().map(|(index, priority)| {
                format!(
                    "{} P{}{}",
                    if index == flow.priority_index {
                        "›"
                    } else {
                        " "
                    },
                    priority,
                    if *priority == 1 { "  default" } else { "" }
                )
            }));
            lines.push(
                "j/k or ↑/↓ select · Enter create · Backspace previous · Esc cancel".to_string(),
            );
        }
    }
    if let Some(status) = status {
        lines.push(String::new());
        lines.push(format!("Error: {status}"));
    }
    draw_floating_modal(
        out,
        width,
        height,
        &format!(" Add child issue · {}/4 ", flow.step.number()),
        &lines,
    )
}

fn draw_add_issue_cancel_confirmation(
    out: &mut impl Write,
    width: u16,
    height: u16,
) -> io::Result<()> {
    draw_floating_modal(
        out,
        width,
        height,
        " Discard new issue? ",
        &[
            "The issue has not been created.".to_string(),
            String::new(),
            "[y] Discard    [n/Esc] Keep editing".to_string(),
        ],
    )
}

fn draw_floating_modal(
    out: &mut impl Write,
    width: u16,
    height: u16,
    title: &str,
    lines: &[String],
) -> io::Result<()> {
    if width == 0 || height == 0 {
        return Ok(());
    }
    if width < 4 || height < 3 {
        let fallback = lines.first().map(String::as_str).unwrap_or(title);
        return queue!(
            out,
            MoveTo(0, height - 1),
            SetAttribute(Attribute::Bold),
            Print(truncate(fallback, width as usize)),
            SetAttribute(Attribute::Reset)
        );
    }

    let visible_lines = lines.len().min(height.saturating_sub(2) as usize);
    let desired_width = lines
        .iter()
        .take(visible_lines)
        .map(|line| line.width())
        .max()
        .unwrap_or(0)
        .max(title.width())
        .saturating_add(4);
    let modal_width = desired_width.min(width as usize).max(4);
    let modal_height = visible_lines + 2;
    let left = (width as usize - modal_width) / 2;
    let top = (height as usize - modal_height) / 2;
    let inner_width = modal_width - 2;
    let top_border = if title.width() <= inner_width {
        format!(
            "╭{title}{}╮",
            "─".repeat(inner_width.saturating_sub(title.width()))
        )
    } else {
        format!("╭{}╮", "─".repeat(inner_width))
    };
    draw_modal_line(out, left, top, &top_border, modal_width, true)?;
    for (index, line) in lines.iter().take(visible_lines).enumerate() {
        let content_width = inner_width.saturating_sub(2);
        let content = truncate(line, content_width);
        let body = format!(
            "│ {}{} │",
            content,
            " ".repeat(content_width.saturating_sub(content.width()))
        );
        draw_modal_line(out, left, top + index + 1, &body, modal_width, false)?;
    }
    let bottom_border = format!("╰{}╯", "─".repeat(inner_width));
    draw_modal_line(
        out,
        left,
        top + modal_height - 1,
        &bottom_border,
        modal_width,
        true,
    )
}

fn draw_modal_line(
    out: &mut impl Write,
    x: usize,
    y: usize,
    line: &str,
    width: usize,
    border: bool,
) -> io::Result<()> {
    queue!(
        out,
        MoveTo(x as u16, y as u16),
        SetBackgroundColor(MODAL_BACKGROUND),
        SetForegroundColor(if border { Color::Cyan } else { Color::White }),
        SetAttribute(if border {
            Attribute::Bold
        } else {
            Attribute::NormalIntensity
        }),
        Print(pad(line, width)),
        ResetColor,
        SetAttribute(Attribute::Reset)
    )?;
    Ok(())
}

fn detail_lines(issue: &Issue, width: usize, selected_dependency: usize) -> Vec<DetailLine> {
    let mut lines = Vec::new();
    let heading = format!(
        "{}  [P{}]  {}  {}",
        issue.id,
        issue.priority,
        issue.status,
        short_type(&issue.issue_type)
    );
    lines.push(DetailLine {
        text: heading,
        style: LineStyle::Header,
        dependency: None,
    });
    lines.push(blank());

    for line in wrap(&issue.title, width) {
        lines.push(DetailLine {
            text: line,
            style: LineStyle::Bold,
            dependency: None,
        });
    }

    if !issue.description.trim().is_empty() {
        lines.push(blank());
        for line in wrap(&issue.description, width) {
            lines.push(DetailLine {
                text: line,
                style: LineStyle::Plain,
                dependency: None,
            });
        }
    }

    lines.push(blank());
    lines.push(DetailLine {
        text: format!("Created:  {}", display_timestamp(&issue.created_at)),
        style: LineStyle::Dim,
        dependency: None,
    });
    lines.push(DetailLine {
        text: format!("Updated:  {}", display_timestamp(&issue.updated_at)),
        style: LineStyle::Dim,
        dependency: None,
    });
    lines.push(blank());
    lines.push(DetailLine {
        text: "Dependencies:".to_string(),
        style: LineStyle::Bold,
        dependency: None,
    });

    if issue.dependencies.is_empty() {
        lines.push(DetailLine {
            text: "  none".to_string(),
            style: LineStyle::Dim,
            dependency: None,
        });
    } else {
        for (index, dependency) in issue.dependencies.iter().enumerate() {
            let dep_type = if dependency.dependency_type.is_empty() {
                String::new()
            } else {
                format!(" ({})", dependency.dependency_type)
            };
            lines.push(DetailLine {
                text: format!(
                    "  → {} [{}] {}{}",
                    dependency.id, dependency.status, dependency.title, dep_type
                ),
                style: if index == selected_dependency {
                    LineStyle::Selected
                } else {
                    LineStyle::Dependency
                },
                dependency: Some(index),
            });
        }
    }
    lines
}

fn blank() -> DetailLine {
    DetailLine {
        text: String::new(),
        style: LineStyle::Plain,
        dependency: None,
    }
}

fn apply_line_style(out: &mut impl Write, style: LineStyle) -> io::Result<()> {
    match style {
        LineStyle::Plain => {}
        LineStyle::Bold => queue!(out, SetAttribute(Attribute::Bold))?,
        LineStyle::Dim => queue!(out, SetAttribute(Attribute::Dim))?,
        LineStyle::Header => queue!(
            out,
            SetForegroundColor(Color::Cyan),
            SetAttribute(Attribute::Bold)
        )?,
        LineStyle::Selected => queue!(
            out,
            SetForegroundColor(Color::Cyan),
            SetBackgroundColor(HIGHLIGHT_BACKGROUND)
        )?,
        LineStyle::Dependency => queue!(out, SetForegroundColor(Color::Cyan))?,
    }
    Ok(())
}

fn write_table_row(
    out: &mut impl Write,
    y: u16,
    cells: &[&str; 5],
    widths: [usize; 5],
    state: Option<RowState>,
) -> io::Result<()> {
    queue!(
        out,
        MoveTo(0, y),
        Clear(ClearType::CurrentLine),
        ResetColor,
        SetAttribute(Attribute::Reset)
    )?;
    let header = state.is_none();
    let selected = state.as_ref().is_some_and(|state| state.selected);
    let highlighted = header || selected;
    if highlighted {
        queue!(out, SetBackgroundColor(HIGHLIGHT_BACKGROUND))?;
    }
    if header {
        queue!(out, SetAttribute(Attribute::Bold))?;
    }

    for index in 0..5 {
        if let Some(state) = &state {
            match index {
                1 => queue!(
                    out,
                    SetForegroundColor(priority_color(state.issue.priority))
                )?,
                2 => queue!(out, SetForegroundColor(status_color(&state.issue.status)))?,
                3 => queue!(out, SetForegroundColor(type_color(&state.issue.issue_type)))?,
                _ => {}
            }
        }
        queue!(out, Print(pad(cells[index], widths[index])))?;
        if !header {
            if selected {
                queue!(out, SetForegroundColor(Color::Reset))?;
            } else {
                queue!(out, ResetColor)?;
            }
        }
        if index < 4 {
            write_separator(out, highlighted)?;
            if header {
                queue!(out, SetAttribute(Attribute::Bold))?;
            }
        }
    }
    queue!(out, ResetColor, SetAttribute(Attribute::Reset))?;
    Ok(())
}

fn write_separator(out: &mut impl Write, highlighted: bool) -> io::Result<()> {
    queue!(
        out,
        SetForegroundColor(Color::DarkGrey),
        SetAttribute(Attribute::Dim),
        Print(SEPARATOR),
        SetAttribute(Attribute::NormalIntensity),
    )?;
    if highlighted {
        queue!(out, SetForegroundColor(Color::Reset))?;
    } else {
        queue!(out, ResetColor)?;
    }
    Ok(())
}

fn table_widths(width: usize, id_hint: usize) -> [usize; 5] {
    let usable = width.saturating_sub(SEPARATOR.width() * 4);
    let mut id = id_hint.clamp(8, 28);
    let mut priority = 8;
    let mut status = 12;
    let issue_type = 4;
    let minimum_name = 12;

    while id + priority + status + issue_type + minimum_name > usable && id > 6 {
        id -= 1;
    }
    while id + priority + status + issue_type + minimum_name > usable && status > 6 {
        status -= 1;
    }
    while id + priority + status + issue_type + minimum_name > usable && priority > 4 {
        priority -= 1;
    }
    if id + priority + status + issue_type > usable {
        let base = usable / 5;
        return [base, base, base, base, usable.saturating_sub(base * 4)];
    }
    [
        id,
        priority,
        status,
        issue_type,
        usable - id - priority - status - issue_type,
    ]
}

fn priority_color(priority: i32) -> Color {
    match priority {
        0 => Color::Red,
        1 => Color::Yellow,
        2 => Color::Green,
        3 => Color::Blue,
        _ => Color::DarkGrey,
    }
}

fn status_color(status: &str) -> Color {
    match status {
        "open" => Color::Green,
        "in_progress" | "in-progress" => Color::Yellow,
        "blocked" => Color::Red,
        "closed" => Color::DarkGrey,
        "deferred" => Color::Blue,
        _ => Color::White,
    }
}

fn type_color(issue_type: &str) -> Color {
    match issue_type {
        "feature" | "feat" => Color::Cyan,
        "epic" => Color::Magenta,
        "bug" => Color::Red,
        "task" => Color::Blue,
        "chore" => Color::Yellow,
        "merge-request" => Color::Green,
        _ => Color::DarkGrey,
    }
}

pub fn short_type(issue_type: &str) -> String {
    match issue_type {
        "feature" | "feat" => "feat".to_string(),
        "epic" => "epic".to_string(),
        "bug" => "bug".to_string(),
        "task" => "task".to_string(),
        "chore" => "chor".to_string(),
        "merge-request" => "mr".to_string(),
        other => truncate(other, 4),
    }
}

fn display_timestamp(timestamp: &str) -> String {
    if timestamp.len() >= 16 && timestamp.as_bytes().get(10) == Some(&b'T') {
        format!("{} {}", &timestamp[..10], &timestamp[11..16])
    } else if timestamp.is_empty() {
        "—".to_string()
    } else {
        timestamp.to_string()
    }
}

fn pad(value: &str, width: usize) -> String {
    let value = truncate(value, width);
    let used = value.width();
    format!("{value}{}", " ".repeat(width.saturating_sub(used)))
}

pub fn truncate(value: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if value.width() <= width {
        return value.to_string();
    }
    if width == 1 {
        return "…".to_string();
    }
    let mut output = String::new();
    let mut used = 0;
    for character in value.chars() {
        let character_width = character.width().unwrap_or(0);
        if used + character_width > width - 1 {
            break;
        }
        output.push(character);
        used += character_width;
    }
    output.push('…');
    output
}

fn wrap(value: &str, width: usize) -> Vec<String> {
    if value.is_empty() {
        return vec![String::new()];
    }
    let width = width.max(1);
    let mut result = Vec::new();
    for paragraph in value.lines() {
        if paragraph.trim().is_empty() {
            result.push(String::new());
            continue;
        }
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let next_width = if line.is_empty() {
                word.width()
            } else {
                line.width() + 1 + word.width()
            };
            if next_width <= width {
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(word);
            } else {
                if !line.is_empty() {
                    result.push(line);
                }
                line = word.to_string();
                while line.width() > width {
                    let mut piece = String::new();
                    let mut used = 0;
                    let mut consumed = 0;
                    for character in line.chars() {
                        let character_width = character.width().unwrap_or(0);
                        if consumed > 0 && used + character_width > width {
                            break;
                        }
                        piece.push(character);
                        used += character_width;
                        consumed += 1;
                        if used >= width {
                            break;
                        }
                    }
                    result.push(piece);
                    line = line.chars().skip(consumed).collect();
                }
            }
        }
        if !line.is_empty() {
            result.push(line);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Issue, IssueGraph};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn close_confirmation_renders_as_a_floating_bordered_modal() {
        let mut app = App::new(IssueGraph::new(
            vec![Issue {
                id: "task-1".into(),
                title: "A task".into(),
                ..Issue::default()
            }],
            vec![],
        ));
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

        let mut output = Vec::new();
        draw(&mut app, &mut output, 80, 20).unwrap();
        let rendered = String::from_utf8_lossy(&output);

        assert!(rendered.contains("╭ Confirm close "));
        assert!(rendered.contains("Close issue task-1?"));
        assert!(rendered.contains("[y] Yes    [n/Esc] No"));
        assert!(rendered.contains('╰'));
    }

    #[test]
    fn add_issue_flow_and_discard_confirmation_render_as_modals() {
        let mut app = App::new(IssueGraph::new(
            vec![Issue {
                id: "task-1".into(),
                title: "A task".into(),
                ..Issue::default()
            }],
            vec![],
        ));
        app.handle_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE));

        let mut output = Vec::new();
        draw(&mut app, &mut output, 90, 24).unwrap();
        let rendered = String::from_utf8_lossy(&output);
        assert!(rendered.contains("Add child issue · 1/4"));
        assert!(rendered.contains("Parent: task-1"));
        assert!(rendered.contains("e $EDITOR"));

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        output.clear();
        draw(&mut app, &mut output, 90, 24).unwrap();
        let rendered = String::from_utf8_lossy(&output);
        assert!(rendered.contains("Discard new issue?"));
        assert!(rendered.contains("[y] Discard"));
    }

    #[test]
    fn status_message_replaces_the_footer_hints() {
        let mut app = App::new(IssueGraph::new(
            vec![Issue {
                id: "task-1".into(),
                title: "A task".into(),
                ..Issue::default()
            }],
            vec![],
        ));
        app.set_status("bd close failed: boom".to_string());

        let mut output = Vec::new();
        draw(&mut app, &mut output, 80, 20).unwrap();
        let rendered = String::from_utf8_lossy(&output);

        assert!(rendered.contains("bd close failed: boom"));
        assert!(!rendered.contains("q quit"));
    }

    #[test]
    fn type_labels_are_never_wider_than_four() {
        for issue_type in ["feature", "epic", "bug", "task", "chore", "molecule"] {
            assert!(short_type(issue_type).width() <= 4);
        }
    }

    #[test]
    fn primary_types_have_distinct_colors() {
        let colors = ["feature", "epic", "bug", "task"].map(type_color);
        for (index, color) in colors.iter().enumerate() {
            assert!(!colors[..index].contains(color));
        }
    }

    #[test]
    fn table_highlights_use_a_background_without_reversing_label_colors() {
        let mut background = Vec::new();
        queue!(background, SetBackgroundColor(HIGHLIGHT_BACKGROUND)).unwrap();
        let mut reverse = Vec::new();
        queue!(reverse, SetAttribute(Attribute::Reverse)).unwrap();
        let mut priority = Vec::new();
        queue!(priority, SetForegroundColor(priority_color(0))).unwrap();
        let mut status = Vec::new();
        queue!(status, SetForegroundColor(status_color("open"))).unwrap();
        let mut issue_type = Vec::new();
        queue!(issue_type, SetForegroundColor(type_color("feature"))).unwrap();

        let issue = Issue {
            priority: 0,
            status: "open".into(),
            issue_type: "feature".into(),
            ..Issue::default()
        };
        let mut selected = Vec::new();
        write_table_row(
            &mut selected,
            1,
            &["issue-1", "P0", "open", "feat", "Example"],
            [8, 8, 8, 4, 12],
            Some(RowState {
                selected: true,
                issue: &issue,
            }),
        )
        .unwrap();
        let mut header = Vec::new();
        write_table_row(
            &mut header,
            0,
            &["ID", "PRIORITY", "STATUS", "TYPE", "NAME"],
            [8, 8, 8, 4, 12],
            None,
        )
        .unwrap();

        assert!(contains_bytes(&selected, &background));
        assert!(contains_bytes(&selected, &priority));
        assert!(contains_bytes(&selected, &status));
        assert!(contains_bytes(&selected, &issue_type));
        assert!(!contains_bytes(&selected, &reverse));
        assert!(contains_bytes(&header, &background));
        assert!(!contains_bytes(&header, &reverse));
    }

    #[test]
    fn truncation_respects_unicode_width() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("界界界", 5), "界界…");
    }

    #[test]
    fn wraps_description_to_terminal_width() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
    }

    fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }
}

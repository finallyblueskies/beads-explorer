use crate::app::{App, Screen};
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

pub fn draw(app: &mut App, out: &mut impl Write, width: u16, height: u16) -> io::Result<()> {
    queue!(out, BeginSynchronizedUpdate, MoveTo(0, 0))?;
    match app.screen() {
        Screen::Tree => draw_tree(app, out, width, height)?,
        Screen::Detail => draw_detail(app, out, width, height)?,
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
        true,
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
                false,
                Some((
                    position == app.cursor,
                    issue.priority,
                    issue.status.as_str(),
                    issue.issue_type.as_str(),
                )),
            )?;
        }
        content_end = visible as u16 + 1;
    }
    queue!(
        out,
        MoveTo(0, content_end),
        Clear(ClearType::FromCursorDown)
    )?;

    let footer = if let Some(query) = app.search_query() {
        format!(
            "/{query}  · {} match{} · ↑/↓ select · Enter open · Esc cancel",
            app.rows.len(),
            if app.rows.len() == 1 { "" } else { "es" }
        )
    } else {
        format!(
            "{} issue{} · / go to · j/k move · h/l fold · Enter open · q quit",
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

    let footer = if app.is_confirming_close() {
        format!("Close issue {}? y confirm · n/Esc cancel", issue.id)
    } else if app.can_close_current_issue() {
        "j/k dependency · Enter open · e description · et title · x close · Backspace back · Esc tree · q quit"
            .to_string()
    } else {
        "j/k dependency · Enter open · e description · et title · Backspace back · Esc tree · q quit"
            .to_string()
    };
    queue!(
        out,
        MoveTo(0, height - 1),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(if app.is_confirming_close() {
            Color::Yellow
        } else {
            Color::Reset
        }),
        SetAttribute(if app.is_confirming_close() {
            Attribute::Bold
        } else {
            Attribute::Dim
        }),
        Print(truncate(&footer, width)),
        ResetColor,
        SetAttribute(Attribute::Reset),
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
    header: bool,
    state: Option<(bool, i32, &str, &str)>,
) -> io::Result<()> {
    queue!(
        out,
        MoveTo(0, y),
        Clear(ClearType::CurrentLine),
        ResetColor,
        SetAttribute(Attribute::Reset)
    )?;
    let selected = state.is_some_and(|state| state.0);
    let highlighted = header || selected;
    if highlighted {
        queue!(out, SetBackgroundColor(HIGHLIGHT_BACKGROUND))?;
    }
    if header {
        queue!(out, SetAttribute(Attribute::Bold))?;
    }

    for index in 0..5 {
        if !header {
            if index == 1 {
                let priority = state.map(|state| state.1).unwrap_or(4);
                queue!(out, SetForegroundColor(priority_color(priority)))?;
            } else if index == 2 {
                let status = state.map(|state| state.2).unwrap_or("");
                queue!(out, SetForegroundColor(status_color(status)))?;
            } else if index == 3 {
                let issue_type = state.map(|state| state.3).unwrap_or("");
                queue!(out, SetForegroundColor(type_color(issue_type)))?;
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

        let mut selected = Vec::new();
        write_table_row(
            &mut selected,
            1,
            &["issue-1", "P0", "open", "feat", "Example"],
            [8, 8, 8, 4, 12],
            false,
            Some((true, 0, "open", "feature")),
        )
        .unwrap();
        let mut header = Vec::new();
        write_table_row(
            &mut header,
            0,
            &["ID", "PRIORITY", "STATUS", "TYPE", "NAME"],
            [8, 8, 8, 4, 12],
            true,
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

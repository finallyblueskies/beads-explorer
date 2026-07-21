use crate::model::{AddIssueDraft, Dependency, EditField, Issue, IssueGraph};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashSet;
use std::time::{Duration, Instant};

const EDIT_SEQUENCE_TIMEOUT: Duration = Duration::from_millis(500);

pub const ISSUE_TYPES: [&str; 6] = ["task", "bug", "feature", "epic", "chore", "decision"];
pub const PRIORITIES: [i32; 5] = [0, 1, 2, 3, 4];
pub const STATUSES: [&str; 5] = ["open", "in_progress", "blocked", "deferred", "closed"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    pub issue_id: String,
    pub path: Vec<String>,
    pub prefix: String,
    pub cycle: bool,
}

impl TreeRow {
    /// The synthetic first row of the tree; Enter on it creates a top-level issue.
    fn create_entry() -> Self {
        Self {
            issue_id: String::new(),
            path: Vec::new(),
            prefix: String::new(),
            cycle: false,
        }
    }

    pub fn is_create_entry(&self) -> bool {
        self.path.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailFrame {
    pub issue_id: String,
    pub dependency_cursor: usize,
    pub scroll: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Tree,
    Detail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddIssueStep {
    Title,
    Description,
    IssueType,
    Priority,
}

impl AddIssueStep {
    pub fn number(self) -> usize {
        match self {
            Self::Title => 1,
            Self::Description => 2,
            Self::IssueType => 3,
            Self::Priority => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddIssueField {
    Title,
    Description,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerKind {
    Status,
    Priority,
}

/// A floating single-choice menu for updating the current issue in place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picker {
    pub kind: PickerKind,
    pub issue_id: String,
    pub index: usize,
}

impl Picker {
    pub fn options(&self) -> Vec<String> {
        match self.kind {
            PickerKind::Status => STATUSES
                .iter()
                .map(|status| (*status).to_string())
                .collect(),
            PickerKind::Priority => PRIORITIES
                .iter()
                .map(|priority| format!("P{priority}"))
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddIssueFlow {
    pub parent_id: Option<String>,
    pub title: String,
    pub description: String,
    pub step: AddIssueStep,
    pub issue_type_index: usize,
    pub priority_index: usize,
    confirming_cancel: bool,
}

impl AddIssueFlow {
    pub fn issue_type(&self) -> &'static str {
        ISSUE_TYPES[self.issue_type_index]
    }

    pub fn priority(&self) -> i32 {
        PRIORITIES[self.priority_index]
    }

    pub fn is_confirming_cancel(&self) -> bool {
        self.confirming_cancel
    }

    fn draft(&self) -> AddIssueDraft {
        AddIssueDraft {
            parent_id: self.parent_id.clone(),
            title: self.title.trim().to_string(),
            description: self.description.trim_end().to_string(),
            issue_type: self.issue_type().to_string(),
            priority: self.priority(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    CloseIssue(String),
    Edit(EditField),
    EditAddIssue(AddIssueField),
    CreateIssue(AddIssueDraft),
    SetStatus(String, &'static str),
    SetPriority(String, i32),
}

/// The mutually exclusive input modes layered over the tree/detail screens.
enum Mode {
    Normal,
    Search {
        query: String,
        origin_cursor: usize,
        origin_scroll: usize,
    },
    ConfirmClose(String),
    AddIssue(AddIssueFlow),
    Picker(Picker),
}

pub struct App {
    pub graph: IssueGraph,
    pub rows: Vec<TreeRow>,
    pub cursor: usize,
    pub scroll: usize,
    pub viewport: usize,
    mode: Mode,
    tree_rows: Vec<TreeRow>,
    expanded: HashSet<Vec<String>>,
    history: Vec<DetailFrame>,
    edit_key_started: Option<Instant>,
    status_message: Option<String>,
}

impl App {
    pub fn new(graph: IssueGraph) -> Self {
        let mut app = Self {
            graph,
            rows: Vec::new(),
            cursor: 0,
            scroll: 0,
            viewport: 1,
            mode: Mode::Normal,
            tree_rows: Vec::new(),
            expanded: HashSet::new(),
            history: Vec::new(),
            edit_key_started: None,
            status_message: None,
        };
        app.rebuild_rows();
        // Start on the first issue rather than the synthetic create entry.
        app.cursor = 1.min(app.rows.len() - 1);
        app
    }

    pub fn screen(&self) -> Screen {
        if self.history.is_empty() {
            Screen::Tree
        } else {
            Screen::Detail
        }
    }

    pub fn current_tree_issue(&self) -> Option<&Issue> {
        self.rows
            .get(self.cursor)
            .and_then(|row| self.graph.issue(&row.issue_id))
    }

    pub fn search_query(&self) -> Option<&str> {
        match &self.mode {
            Mode::Search { query, .. } => Some(query),
            _ => None,
        }
    }

    pub fn current_detail_issue(&self) -> Option<&Issue> {
        self.history
            .last()
            .and_then(|frame| self.graph.issue(&frame.issue_id))
    }

    /// The issue the edit/status/priority/close keys act on: the selected tree
    /// row or the issue in the task view, depending on the visible screen.
    pub fn current_issue(&self) -> Option<&Issue> {
        match self.screen() {
            Screen::Tree => self.current_tree_issue(),
            Screen::Detail => self.current_detail_issue(),
        }
    }

    pub fn detail_frame(&self) -> Option<&DetailFrame> {
        self.history.last()
    }

    pub fn detail_frame_mut(&mut self) -> Option<&mut DetailFrame> {
        self.history.last_mut()
    }

    pub fn is_confirming_close(&self) -> bool {
        matches!(self.mode, Mode::ConfirmClose(_))
    }

    pub fn closing_issue_id(&self) -> Option<&str> {
        match &self.mode {
            Mode::ConfirmClose(issue_id) => Some(issue_id),
            _ => None,
        }
    }

    pub fn add_issue_flow(&self) -> Option<&AddIssueFlow> {
        match &self.mode {
            Mode::AddIssue(flow) => Some(flow),
            _ => None,
        }
    }

    pub fn picker(&self) -> Option<&Picker> {
        match &self.mode {
            Mode::Picker(picker) => Some(picker),
            _ => None,
        }
    }

    pub fn set_add_issue_field(&mut self, field: AddIssueField, value: String) {
        let Mode::AddIssue(flow) = &mut self.mode else {
            return;
        };
        match field {
            AddIssueField::Title => {
                flow.title = value
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
            }
            AddIssueField::Description => flow.description = value.trim_end().to_string(),
        }
    }

    pub fn add_issue_field(&self, field: AddIssueField) -> Option<&str> {
        self.add_issue_flow().map(|flow| match field {
            AddIssueField::Title => flow.title.as_str(),
            AddIssueField::Description => flow.description.as_str(),
        })
    }

    pub fn finish_add_issue(&mut self) {
        self.mode = Mode::Normal;
    }

    pub fn status_message(&self) -> Option<&str> {
        self.status_message.as_deref()
    }

    pub fn set_status(&mut self, message: String) {
        self.status_message = Some(message);
    }

    pub fn clear_status(&mut self) {
        self.status_message = None;
    }

    pub fn can_close_current_issue(&self) -> bool {
        self.current_detail_issue()
            .is_some_and(|issue| self.graph.is_listed(&issue.id))
    }

    pub fn row_is_expanded(&self, row: &TreeRow) -> bool {
        self.expanded.contains(&row.path)
    }

    pub fn row_has_children(&self, row: &TreeRow) -> bool {
        !row.cycle && !self.graph.tree_children(&row.issue_id).is_empty()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        self.status_message = None;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.mode = Mode::Normal;
            self.edit_key_started = None;
            return Action::Quit;
        }
        match self.mode {
            Mode::AddIssue(_) => self.handle_add_issue_key(key),
            Mode::ConfirmClose(_) => self.handle_confirm_close_key(key),
            Mode::Search { .. } => self.handle_search_key(key),
            Mode::Picker(_) => self.handle_picker_key(key),
            Mode::Normal => match self.screen() {
                Screen::Tree => self.handle_tree_key(key),
                Screen::Detail => self.handle_detail_key(key),
            },
        }
    }

    fn handle_confirm_close_key(&mut self, key: KeyEvent) -> Action {
        let Mode::ConfirmClose(issue_id) = &self.mode else {
            return Action::None;
        };
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                let issue_id = issue_id.clone();
                self.mode = Mode::Normal;
                Action::CloseIssue(issue_id)
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                self.mode = Mode::Normal;
                Action::None
            }
            _ => Action::None,
        }
    }

    pub fn pending_key_timeout(&self) -> Option<Duration> {
        self.edit_key_started
            .map(|started| EDIT_SEQUENCE_TIMEOUT.saturating_sub(started.elapsed()))
    }

    pub fn flush_pending_key(&mut self) -> Action {
        if self.edit_key_started.take().is_some() {
            Action::Edit(EditField::Description)
        } else {
            Action::None
        }
    }

    /// Resolves a pending `e` chord: `et` edits the title, `ee` (or the
    /// timeout) edits the description, anything else cancels the chord and is
    /// handled normally by the caller.
    fn resolve_edit_chord(&mut self, key: &KeyEvent) -> Option<Action> {
        let started = self.edit_key_started.take()?;
        if started.elapsed() >= EDIT_SEQUENCE_TIMEOUT {
            return Some(Action::Edit(EditField::Description));
        }
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        if plain && key.code == KeyCode::Char('t') {
            return Some(Action::Edit(EditField::Title));
        }
        if plain && key.code == KeyCode::Char('e') {
            return Some(Action::Edit(EditField::Description));
        }
        None
    }

    fn handle_tree_key(&mut self, key: KeyEvent) -> Action {
        if let Some(action) = self.resolve_edit_chord(&key) {
            return action;
        }
        if key.code == KeyCode::Char('/') {
            self.start_search();
            return Action::None;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('d') if ctrl => self.move_cursor(self.viewport as isize / 2),
            KeyCode::Char('u') if ctrl => self.move_cursor(-(self.viewport as isize) / 2),
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1),
            KeyCode::PageDown => self.move_cursor(self.viewport as isize),
            KeyCode::PageUp => self.move_cursor(-(self.viewport as isize)),
            KeyCode::Char('g') | KeyCode::Home => self.cursor = 0,
            KeyCode::Char('G') | KeyCode::End => self.cursor = self.rows.len() - 1,
            KeyCode::Tab => self.toggle(),
            KeyCode::Char('l') | KeyCode::Right => self.expand_or_enter(),
            KeyCode::Char('h') | KeyCode::Left => self.collapse_or_parent(),
            KeyCode::Enter if self.selected_row_is_create_entry() => self.start_add_issue(None),
            KeyCode::Enter => self.open_selected_issue(),
            KeyCode::Char('e') if self.current_tree_issue().is_some() => {
                self.edit_key_started = Some(Instant::now());
            }
            KeyCode::Char('s') => self.start_picker(PickerKind::Status),
            KeyCode::Char('p') => self.start_picker(PickerKind::Priority),
            KeyCode::Char('x') => self.start_close_confirmation(),
            KeyCode::Char('+') => {
                let parent_id = self.current_tree_issue().map(|issue| issue.id.clone());
                self.start_add_issue(parent_id);
            }
            KeyCode::Char('q') | KeyCode::Esc => return Action::Quit,
            _ => {}
        }
        Action::None
    }

    fn selected_row_is_create_entry(&self) -> bool {
        self.rows
            .get(self.cursor)
            .is_some_and(TreeRow::is_create_entry)
    }

    fn handle_search_key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.cancel_search(),
            KeyCode::Enter => self.open_search_result(),
            KeyCode::Char('d') if ctrl => self.move_cursor(self.viewport as isize / 2),
            KeyCode::Char('u') if ctrl => {
                if let Mode::Search { query, .. } = &mut self.mode {
                    query.clear();
                }
                self.rebuild_search_rows();
            }
            KeyCode::Down => self.move_cursor(1),
            KeyCode::Up => self.move_cursor(-1),
            KeyCode::PageDown => self.move_cursor(self.viewport as isize),
            KeyCode::PageUp => self.move_cursor(-(self.viewport as isize)),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.rows.len().saturating_sub(1),
            KeyCode::Backspace => {
                if let Mode::Search { query, .. } = &mut self.mode {
                    query.pop();
                }
                self.rebuild_search_rows();
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Mode::Search { query, .. } = &mut self.mode {
                    query.push(character);
                }
                self.rebuild_search_rows();
            }
            _ => {}
        }
        Action::None
    }

    fn handle_detail_key(&mut self, key: KeyEvent) -> Action {
        if let Some(action) = self.resolve_edit_chord(&key) {
            return action;
        }

        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_dependency_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_dependency_cursor(-1),
            KeyCode::Char('g') | KeyCode::Home => {
                if let Some(frame) = self.history.last_mut() {
                    frame.dependency_cursor = 0;
                }
            }
            KeyCode::Char('G') | KeyCode::End => {
                let last = self
                    .current_detail_issue()
                    .map(|issue| issue.dependencies.len().saturating_sub(1))
                    .unwrap_or(0);
                if let Some(frame) = self.history.last_mut() {
                    frame.dependency_cursor = last;
                }
            }
            KeyCode::Enter => self.open_selected_dependency(),
            KeyCode::Char('e') if self.current_detail_issue().is_some() => {
                self.edit_key_started = Some(Instant::now());
            }
            KeyCode::Char('s') => self.start_picker(PickerKind::Status),
            KeyCode::Char('p') => self.start_picker(PickerKind::Priority),
            KeyCode::Char('x') if self.can_close_current_issue() => {
                self.start_close_confirmation();
            }
            KeyCode::Char('+') if self.current_detail_issue().is_some() => {
                let parent_id = self.current_detail_issue().map(|issue| issue.id.clone());
                self.start_add_issue(parent_id);
            }
            KeyCode::Backspace => {
                self.history.pop();
            }
            KeyCode::Esc => self.history.clear(),
            KeyCode::Char('q') => return Action::Quit,
            _ => {}
        }
        Action::None
    }

    fn handle_add_issue_key(&mut self, key: KeyEvent) -> Action {
        let Mode::AddIssue(flow) = &mut self.mode else {
            return Action::None;
        };

        if flow.confirming_cancel {
            match key.code {
                KeyCode::Char('y' | 'Y') => self.mode = Mode::Normal,
                KeyCode::Char('n' | 'N') | KeyCode::Esc => flow.confirming_cancel = false,
                _ => {}
            }
            return Action::None;
        }

        if key.code == KeyCode::Esc {
            flow.confirming_cancel = true;
            return Action::None;
        }

        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match flow.step {
            AddIssueStep::Title => match key.code {
                KeyCode::Enter if !flow.title.trim().is_empty() => {
                    flow.step = AddIssueStep::Description;
                }
                KeyCode::Backspace => {
                    flow.title.pop();
                }
                KeyCode::Char('e') if plain && flow.title.is_empty() => {
                    return Action::EditAddIssue(AddIssueField::Title);
                }
                KeyCode::Char(character) if plain => flow.title.push(character),
                _ => {}
            },
            AddIssueStep::Description => match key.code {
                KeyCode::Enter => flow.step = AddIssueStep::IssueType,
                KeyCode::Backspace => {
                    flow.description.pop();
                }
                KeyCode::Char('e') if plain && flow.description.is_empty() => {
                    return Action::EditAddIssue(AddIssueField::Description);
                }
                KeyCode::Char(character) if plain => flow.description.push(character),
                _ => {}
            },
            AddIssueStep::IssueType => match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    flow.issue_type_index = (flow.issue_type_index + 1).min(ISSUE_TYPES.len() - 1);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    flow.issue_type_index = flow.issue_type_index.saturating_sub(1);
                }
                KeyCode::Char('g') | KeyCode::Home => flow.issue_type_index = 0,
                KeyCode::Char('G') | KeyCode::End => {
                    flow.issue_type_index = ISSUE_TYPES.len() - 1;
                }
                KeyCode::Enter => flow.step = AddIssueStep::Priority,
                KeyCode::Backspace => flow.step = AddIssueStep::Description,
                _ => {}
            },
            AddIssueStep::Priority => match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    flow.priority_index = (flow.priority_index + 1).min(PRIORITIES.len() - 1);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    flow.priority_index = flow.priority_index.saturating_sub(1);
                }
                KeyCode::Char('g') | KeyCode::Home => flow.priority_index = 0,
                KeyCode::Char('G') | KeyCode::End => {
                    flow.priority_index = PRIORITIES.len() - 1;
                }
                KeyCode::Enter => return Action::CreateIssue(flow.draft()),
                KeyCode::Backspace => flow.step = AddIssueStep::IssueType,
                _ => {}
            },
        }
        Action::None
    }

    fn handle_picker_key(&mut self, key: KeyEvent) -> Action {
        let Mode::Picker(picker) = &mut self.mode else {
            return Action::None;
        };
        let last = picker.options().len() - 1;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => picker.index = (picker.index + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => picker.index = picker.index.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => picker.index = 0,
            KeyCode::Char('G') | KeyCode::End => picker.index = last,
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => {
                let Mode::Picker(picker) = std::mem::replace(&mut self.mode, Mode::Normal) else {
                    return Action::None;
                };
                return match picker.kind {
                    PickerKind::Status => {
                        Action::SetStatus(picker.issue_id, STATUSES[picker.index])
                    }
                    PickerKind::Priority => {
                        Action::SetPriority(picker.issue_id, PRIORITIES[picker.index])
                    }
                };
            }
            _ => {}
        }
        Action::None
    }

    fn start_picker(&mut self, kind: PickerKind) {
        let Some(issue) = self.current_issue() else {
            return;
        };
        let index = match kind {
            PickerKind::Status => STATUSES
                .iter()
                .position(|status| *status == issue.status)
                .unwrap_or(0),
            PickerKind::Priority => PRIORITIES
                .iter()
                .position(|priority| *priority == issue.priority)
                .unwrap_or(1),
        };
        let issue_id = issue.id.clone();
        self.edit_key_started = None;
        self.mode = Mode::Picker(Picker {
            kind,
            issue_id,
            index,
        });
    }

    fn move_cursor(&mut self, delta: isize) {
        let last = self.rows.len().saturating_sub(1) as isize;
        self.cursor = (self.cursor as isize + delta).clamp(0, last) as usize;
    }

    fn move_dependency_cursor(&mut self, delta: isize) {
        let last = self
            .current_detail_issue()
            .map(|issue| issue.dependencies.len().saturating_sub(1))
            .unwrap_or(0) as isize;
        if let Some(frame) = self.history.last_mut() {
            frame.dependency_cursor =
                (frame.dependency_cursor as isize + delta).clamp(0, last) as usize;
        }
    }

    fn open_selected_issue(&mut self) {
        if let Some(row) = self.rows.get(self.cursor) {
            if row.is_create_entry() {
                return;
            }
            self.history.push(DetailFrame {
                issue_id: row.issue_id.clone(),
                dependency_cursor: 0,
                scroll: 0,
            });
        }
    }

    fn start_close_confirmation(&mut self) {
        let issue_id = self
            .current_issue()
            .filter(|issue| self.graph.is_listed(&issue.id))
            .map(|issue| issue.id.clone());
        if let Some(issue_id) = issue_id {
            self.mode = Mode::ConfirmClose(issue_id);
        }
    }

    fn start_add_issue(&mut self, parent_id: Option<String>) {
        self.edit_key_started = None;
        self.mode = Mode::AddIssue(AddIssueFlow {
            parent_id,
            title: String::new(),
            description: String::new(),
            step: AddIssueStep::Title,
            issue_type_index: 0,
            priority_index: 1,
            confirming_cancel: false,
        });
    }

    pub fn return_to_tree(&mut self) {
        self.history.clear();
        self.mode = Mode::Normal;
        self.edit_key_started = None;
    }

    /// Replace data loaded from `bd` without resetting the user's place in the tree.
    /// Exact paths win; issue IDs are the fallback when dependency changes move a branch.
    pub fn refresh_graph(&mut self, graph: IssueGraph) {
        let (selected_path, selected_issue_id, old_cursor, old_scroll) = if let Mode::Search {
            origin_cursor,
            origin_scroll,
            ..
        } = self.mode
        {
            let cursor = origin_cursor.min(self.tree_rows.len().saturating_sub(1));
            let row = self.tree_rows.get(cursor);
            (
                row.map(|row| row.path.clone()),
                row.map(|row| row.issue_id.clone()),
                cursor,
                origin_scroll,
            )
        } else {
            let row = self.rows.get(self.cursor);
            (
                row.map(|row| row.path.clone()),
                row.map(|row| row.issue_id.clone()),
                self.cursor,
                self.scroll,
            )
        };
        let viewport_offset = old_cursor.saturating_sub(old_scroll);
        let moved_expanded_issue_ids: HashSet<String> = self
            .expanded
            .iter()
            .filter(|path| !Self::tree_path_exists(&graph, path))
            .filter_map(|path| path.last().cloned())
            .collect();

        self.graph = graph;
        // Keep an in-flight add-issue draft; search and close confirmation are
        // transient and rebuilt against stale rows, so they end here.
        if !matches!(self.mode, Mode::AddIssue(_)) {
            self.mode = Mode::Normal;
        }
        self.edit_key_started = None;

        // Paths that left the tree must not linger: a later refresh could
        // recreate one and surprise-expand a branch the user never opened.
        let current_graph = &self.graph;
        self.expanded
            .retain(|path| Self::tree_path_exists(current_graph, path));

        let (rows, restored_expansions) = self.build_rows(Some(&moved_expanded_issue_ids));
        self.expanded.extend(restored_expansions);
        self.rows = rows;
        self.tree_rows.clone_from(&self.rows);

        let exact = selected_path
            .as_ref()
            .and_then(|path| self.rows.iter().position(|row| &row.path == path));
        let by_issue = selected_issue_id.as_ref().and_then(|issue_id| {
            self.rows
                .iter()
                .enumerate()
                .filter(|(_, row)| &row.issue_id == issue_id)
                .min_by_key(|(position, _)| position.abs_diff(old_cursor))
                .map(|(position, _)| position)
        });
        self.cursor = exact
            .or(by_issue)
            .unwrap_or(old_cursor)
            .min(self.rows.len().saturating_sub(1));
        self.scroll = self.cursor.saturating_sub(viewport_offset);

        self.history
            .retain(|frame| self.graph.issue(&frame.issue_id).is_some());
        for frame in &mut self.history {
            let last_dependency = self
                .graph
                .issue(&frame.issue_id)
                .map(|issue| issue.dependencies.len().saturating_sub(1))
                .unwrap_or(0);
            frame.dependency_cursor = frame.dependency_cursor.min(last_dependency);
        }
    }

    fn tree_path_exists(graph: &IssueGraph, path: &[String]) -> bool {
        let Some(root) = path.first() else {
            return false;
        };
        graph.roots().contains(root)
            && path.windows(2).all(|edge| {
                graph
                    .tree_children(&edge[0])
                    .iter()
                    .any(|child| child == &edge[1])
            })
    }

    fn start_search(&mut self) {
        self.mode = Mode::Search {
            query: String::new(),
            origin_cursor: self.cursor,
            origin_scroll: self.scroll,
        };
        self.rebuild_search_rows();
    }

    fn cancel_search(&mut self) {
        let Mode::Search {
            origin_cursor,
            origin_scroll,
            ..
        } = std::mem::replace(&mut self.mode, Mode::Normal)
        else {
            return;
        };
        self.rows.clone_from(&self.tree_rows);
        self.cursor = origin_cursor.min(self.rows.len().saturating_sub(1));
        self.scroll = origin_scroll;
    }

    fn open_search_result(&mut self) {
        let issue_id = self.rows.get(self.cursor).map(|row| row.issue_id.clone());
        self.cancel_search();
        if let Some(issue_id) = issue_id {
            self.history.push(DetailFrame {
                issue_id,
                dependency_cursor: 0,
                scroll: 0,
            });
        }
    }

    fn rebuild_search_rows(&mut self) {
        let Mode::Search { query, .. } = &self.mode else {
            return;
        };
        self.rows = self
            .tree_rows
            .iter()
            .filter(|row| !row.is_create_entry() && fuzzy_match(&row.issue_id, query))
            .cloned()
            .collect();
        self.cursor = 0;
        self.scroll = 0;
    }

    fn open_selected_dependency(&mut self) {
        let dependency = self.selected_dependency().cloned();
        if let Some(dependency) = dependency {
            self.history.push(DetailFrame {
                issue_id: dependency.id,
                dependency_cursor: 0,
                scroll: 0,
            });
        }
    }

    pub fn selected_dependency(&self) -> Option<&Dependency> {
        let frame = self.history.last()?;
        self.graph
            .issue(&frame.issue_id)?
            .dependencies
            .get(frame.dependency_cursor)
    }

    fn toggle(&mut self) {
        let Some(row) = self.rows.get(self.cursor).cloned() else {
            return;
        };
        if !self.row_has_children(&row) {
            return;
        }
        if !self.expanded.remove(&row.path) {
            self.expanded.insert(row.path);
        }
        self.rebuild_rows();
    }

    fn expand_or_enter(&mut self) {
        let Some(row) = self.rows.get(self.cursor).cloned() else {
            return;
        };
        if !self.row_has_children(&row) {
            return;
        }
        if self.expanded.insert(row.path.clone()) {
            self.rebuild_rows();
        } else if self.cursor + 1 < self.rows.len()
            && self.rows[self.cursor + 1].path.starts_with(&row.path)
        {
            self.cursor += 1;
        }
    }

    fn collapse_or_parent(&mut self) {
        let Some(row) = self.rows.get(self.cursor).cloned() else {
            return;
        };
        if self.expanded.remove(&row.path) {
            self.rebuild_rows();
            return;
        }
        if row.path.len() <= 1 {
            return;
        }
        let parent = &row.path[..row.path.len() - 1];
        if let Some(position) = self
            .rows
            .iter()
            .position(|candidate| candidate.path == parent)
        {
            self.cursor = position;
        }
    }

    fn build_rows(
        &self,
        expanded_issue_ids: Option<&HashSet<String>>,
    ) -> (Vec<TreeRow>, HashSet<Vec<String>>) {
        fn walk(
            graph: &IssueGraph,
            expanded: &HashSet<Vec<String>>,
            expanded_issue_ids: Option<&HashSet<String>>,
            path: &[String],
            prefix: &str,
            rows: &mut Vec<TreeRow>,
            restored_expansions: &mut HashSet<Vec<String>>,
        ) {
            let Some(issue_id) = path.last() else {
                return;
            };
            if !expanded.contains(path)
                && !expanded_issue_ids.is_some_and(|ids| ids.contains(issue_id))
            {
                return;
            }
            restored_expansions.insert(path.to_vec());
            let children = graph.tree_children(issue_id);
            let count = children.len();
            for (index, child_id) in children.iter().enumerate() {
                let last = index + 1 == count;
                let cycle = path.iter().any(|id| id == child_id);
                let mut child_path = path.to_vec();
                child_path.push(child_id.clone());
                rows.push(TreeRow {
                    issue_id: child_id.clone(),
                    path: child_path.clone(),
                    prefix: format!("{prefix}{}", if last { "└── " } else { "├── " }),
                    cycle,
                });
                if !cycle {
                    walk(
                        graph,
                        expanded,
                        expanded_issue_ids,
                        &child_path,
                        &format!("{prefix}{}", if last { "    " } else { "│   " }),
                        rows,
                        restored_expansions,
                    );
                }
            }
        }

        let mut rows = vec![TreeRow::create_entry()];
        let mut restored_expansions = HashSet::new();
        for root in self.graph.roots() {
            let path = vec![root.clone()];
            rows.push(TreeRow {
                issue_id: root.clone(),
                path: path.clone(),
                prefix: String::new(),
                cycle: false,
            });
            walk(
                &self.graph,
                &self.expanded,
                expanded_issue_ids,
                &path,
                "",
                &mut rows,
                &mut restored_expansions,
            );
        }
        (rows, restored_expansions)
    }

    fn rebuild_rows(&mut self) {
        let selected_path = self.rows.get(self.cursor).map(|row| row.path.clone());
        let (rows, _) = self.build_rows(None);
        self.rows = rows;
        self.tree_rows.clone_from(&self.rows);

        if let Some(path) = selected_path {
            if let Some(position) = self.rows.iter().position(|row| row.path == path) {
                self.cursor = position;
            }
        }
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
    }
}

fn fuzzy_match(candidate: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }

    let mut search_from = 0;
    let candidate = candidate.to_lowercase();
    let query = query.to_lowercase();
    for needle in query.chars() {
        let Some(remainder) = candidate.get(search_from..) else {
            return false;
        };
        let Some(offset) = remainder.find(needle) else {
            return false;
        };
        search_from += offset + needle.len_utf8();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Dependency, Issue};

    fn graph() -> IssueGraph {
        IssueGraph::new(
            vec![
                Issue {
                    id: "a".into(),
                    title: "A".into(),
                    dependencies: vec![Dependency {
                        id: "b".into(),
                        title: "B".into(),
                        ..Dependency::default()
                    }],
                    ..Issue::default()
                },
                Issue {
                    id: "b".into(),
                    title: "B".into(),
                    ..Issue::default()
                },
            ],
            vec![],
        )
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// The tree rows without the synthetic create entry.
    fn issue_ids(app: &App) -> Vec<&str> {
        app.rows
            .iter()
            .filter(|row| !row.is_create_entry())
            .map(|row| row.issue_id.as_str())
            .collect()
    }

    #[test]
    fn tree_starts_at_first_level_and_expands_like_treec() {
        let mut app = App::new(graph());
        assert!(app.rows[0].is_create_entry());
        assert_eq!(issue_ids(&app), ["a"]);
        assert_eq!(app.cursor, 1, "cursor starts on the first issue");

        app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(issue_ids(&app), ["a", "b"]);

        app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(app.cursor, 2);
        app.handle_key(key(KeyCode::Char('h')));
        assert_eq!(app.cursor, 1);
    }

    #[test]
    fn parent_child_edges_expand_from_parent_to_child() {
        let child = Issue {
            id: "hmb2".into(),
            dependencies: vec![Dependency {
                id: "8gda".into(),
                dependency_type: "parent-child".into(),
                ..Dependency::default()
            }],
            ..Issue::default()
        };
        let parent = Issue {
            id: "8gda".into(),
            ..Issue::default()
        };
        let mut app = App::new(IssueGraph::new(vec![child, parent], vec![]));

        assert_eq!(issue_ids(&app), ["8gda"]);

        app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(issue_ids(&app), ["8gda", "hmb2"]);
    }

    #[test]
    fn task_view_uses_history_and_escape_returns_to_tree() {
        let mut app = App::new(graph());
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen(), Screen::Detail);
        assert_eq!(app.current_detail_issue().unwrap().id, "a");

        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.current_detail_issue().unwrap().id, "b");
        app.handle_key(key(KeyCode::Backspace));
        assert_eq!(app.current_detail_issue().unwrap().id, "a");
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.screen(), Screen::Tree);
    }

    #[test]
    fn plus_starts_add_issue_at_the_selected_tree_location() {
        let mut app = App::new(graph());

        assert_eq!(app.handle_key(key(KeyCode::Char('+'))), Action::None);
        let flow = app.add_issue_flow().unwrap();
        assert_eq!(flow.parent_id.as_deref(), Some("a"));
        assert_eq!(flow.step, AddIssueStep::Title);
        assert_eq!(flow.issue_type(), "task");
        assert_eq!(flow.priority(), 1);
    }

    #[test]
    fn plus_starts_add_issue_at_the_current_task_view_location() {
        let mut app = App::new(graph());
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Enter));

        app.handle_key(key(KeyCode::Char('+')));

        assert_eq!(
            app.add_issue_flow().unwrap().parent_id.as_deref(),
            Some("b")
        );
    }

    #[test]
    fn add_issue_flow_collects_fields_and_emits_a_create_action() {
        let mut app = App::new(graph());
        app.handle_key(key(KeyCode::Char('+')));

        assert_eq!(
            app.handle_key(key(KeyCode::Char('e'))),
            Action::EditAddIssue(AddIssueField::Title)
        );
        app.set_add_issue_field(AddIssueField::Title, "A new child\n".into());
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            app.add_issue_flow().unwrap().step,
            AddIssueStep::Description
        );
        assert_eq!(
            app.handle_key(key(KeyCode::Char('e'))),
            Action::EditAddIssue(AddIssueField::Description)
        );
        app.set_add_issue_field(AddIssueField::Description, "Some detail\n".into());
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));

        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Action::CreateIssue(AddIssueDraft {
                parent_id: Some("a".into()),
                title: "A new child".into(),
                description: "Some detail".into(),
                issue_type: "bug".into(),
                priority: 1,
            })
        );
        assert!(
            app.add_issue_flow().is_some(),
            "failed creates can be retried"
        );
    }

    #[test]
    fn escape_confirms_before_discarding_add_issue_progress() {
        let mut app = App::new(graph());
        app.handle_key(key(KeyCode::Char('+')));
        app.handle_key(key(KeyCode::Char('F')));

        app.handle_key(key(KeyCode::Esc));
        assert!(app
            .add_issue_flow()
            .is_some_and(AddIssueFlow::is_confirming_cancel));
        app.handle_key(key(KeyCode::Esc));
        assert!(!app
            .add_issue_flow()
            .is_some_and(AddIssueFlow::is_confirming_cancel));
        assert_eq!(app.add_issue_flow().unwrap().title, "F");

        app.handle_key(key(KeyCode::Esc));
        app.handle_key(key(KeyCode::Char('y')));
        assert!(app.add_issue_flow().is_none());
        assert_eq!(app.screen(), Screen::Tree);
    }

    #[test]
    fn e_in_task_view_requests_description_edit_after_the_sequence_timeout() {
        let mut app = App::new(graph());
        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.handle_key(key(KeyCode::Char('e'))), Action::None);
        assert_eq!(
            app.flush_pending_key(),
            Action::Edit(EditField::Description)
        );
        assert_eq!(app.current_detail_issue().unwrap().id, "a");
    }

    #[test]
    fn e_in_tree_requests_description_edit_without_leaving_the_tree() {
        let mut app = App::new(graph());

        assert_eq!(app.handle_key(key(KeyCode::Char('e'))), Action::None);
        assert_eq!(
            app.flush_pending_key(),
            Action::Edit(EditField::Description)
        );
        assert_eq!(app.screen(), Screen::Tree);
        assert_eq!(app.current_issue().unwrap().id, "a");
    }

    #[test]
    fn e_then_t_in_tree_requests_title_edit_like_the_task_view() {
        let mut app = App::new(graph());

        assert_eq!(app.handle_key(key(KeyCode::Char('e'))), Action::None);
        assert_eq!(
            app.handle_key(key(KeyCode::Char('t'))),
            Action::Edit(EditField::Title)
        );
        assert_eq!(app.screen(), Screen::Tree);
        assert_eq!(app.current_issue().unwrap().id, "a");
    }

    #[test]
    fn e_then_t_in_task_view_requests_title_edit() {
        let mut app = App::new(graph());
        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.handle_key(key(KeyCode::Char('e'))), Action::None);
        assert_eq!(
            app.handle_key(key(KeyCode::Char('t'))),
            Action::Edit(EditField::Title)
        );
        assert_eq!(app.current_detail_issue().unwrap().id, "a");
    }

    #[test]
    fn e_then_an_unrelated_key_cancels_the_sequence_and_handles_the_key() {
        let mut app = App::new(graph());
        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.handle_key(key(KeyCode::Char('e'))), Action::None);
        assert_eq!(app.handle_key(key(KeyCode::Char('q'))), Action::Quit);
        assert_eq!(app.flush_pending_key(), Action::None);
    }

    #[test]
    fn e_pressed_twice_requests_description_edit_immediately() {
        let mut app = App::new(graph());
        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.handle_key(key(KeyCode::Char('e'))), Action::None);
        assert_eq!(
            app.handle_key(key(KeyCode::Char('e'))),
            Action::Edit(EditField::Description)
        );
    }

    #[test]
    fn status_message_clears_on_the_next_key() {
        let mut app = App::new(graph());
        app.set_status("Closing a…".to_string());
        assert_eq!(app.status_message(), Some("Closing a…"));

        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.status_message(), None);
    }

    #[test]
    fn refresh_forgets_expansion_of_issues_that_left_the_graph() {
        let mut app = App::new(graph());
        app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(issue_ids(&app), ["a", "b"]);

        app.refresh_graph(IssueGraph::new(
            vec![Issue {
                id: "b".into(),
                ..Issue::default()
            }],
            vec![],
        ));
        app.refresh_graph(graph());

        assert_eq!(issue_ids(&app), ["a"], "a must come back collapsed");
        assert!(!app.row_is_expanded(&app.rows[1]));
    }

    #[test]
    fn x_in_task_view_requires_confirmation_before_closing() {
        let mut app = App::new(graph());
        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.handle_key(key(KeyCode::Char('x'))), Action::None);
        assert!(app.is_confirming_close());
        assert_eq!(app.closing_issue_id(), Some("a"));
        assert_eq!(
            app.handle_key(key(KeyCode::Char('y'))),
            Action::CloseIssue("a".into())
        );
        assert!(!app.is_confirming_close());
        assert_eq!(app.current_detail_issue().unwrap().id, "a");
    }

    #[test]
    fn x_in_tree_view_closes_the_selected_issue_after_confirmation() {
        let mut app = App::new(graph());

        assert_eq!(app.handle_key(key(KeyCode::Char('x'))), Action::None);
        assert_eq!(app.closing_issue_id(), Some("a"));
        assert_eq!(
            app.handle_key(key(KeyCode::Char('y'))),
            Action::CloseIssue("a".into())
        );
        assert_eq!(app.screen(), Screen::Tree);
    }

    #[test]
    fn close_confirmation_can_be_cancelled() {
        let mut app = App::new(graph());
        app.handle_key(key(KeyCode::Enter));

        app.handle_key(key(KeyCode::Char('x')));
        assert_eq!(app.handle_key(key(KeyCode::Esc)), Action::None);
        assert!(!app.is_confirming_close());
        assert_eq!(app.screen(), Screen::Detail);
    }

    #[test]
    fn context_issues_cannot_be_closed_again() {
        let mut app = App::new(IssueGraph::new(
            vec![Issue {
                id: "open".into(),
                dependencies: vec![Dependency {
                    id: "closed".into(),
                    status: "closed".into(),
                    ..Dependency::default()
                }],
                ..Issue::default()
            }],
            vec![Issue {
                id: "closed".into(),
                status: "closed".into(),
                ..Issue::default()
            }],
        ));
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Enter));

        assert!(!app.can_close_current_issue());
        assert_eq!(app.handle_key(key(KeyCode::Char('x'))), Action::None);
        assert!(!app.is_confirming_close());
    }

    #[test]
    fn refresh_preserves_tree_state_when_closing_reparents_dependencies() {
        let mut app = App::new(IssueGraph::new(
            vec![
                Issue {
                    id: "a".into(),
                    dependencies: vec![Dependency {
                        id: "b".into(),
                        ..Dependency::default()
                    }],
                    ..Issue::default()
                },
                Issue {
                    id: "b".into(),
                    dependencies: vec![Dependency {
                        id: "c".into(),
                        ..Dependency::default()
                    }],
                    ..Issue::default()
                },
                Issue {
                    id: "c".into(),
                    ..Issue::default()
                },
            ],
            vec![],
        ));
        app.handle_key(key(KeyCode::Char('l')));
        app.handle_key(key(KeyCode::Char('j')));
        app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(issue_ids(&app), ["a", "b", "c"]);

        app.cursor = 1;
        app.scroll = 0;
        app.handle_key(key(KeyCode::Char('x')));
        assert_eq!(
            app.handle_key(key(KeyCode::Char('y'))),
            Action::CloseIssue("a".into())
        );

        // `bd` removes closed a from the open list, making b a root. Its expanded
        // state follows the issue identity so c stays visible on the moved branch.
        app.return_to_tree();
        app.refresh_graph(IssueGraph::new(
            vec![
                Issue {
                    id: "b".into(),
                    dependencies: vec![Dependency {
                        id: "c".into(),
                        ..Dependency::default()
                    }],
                    ..Issue::default()
                },
                Issue {
                    id: "c".into(),
                    ..Issue::default()
                },
            ],
            vec![],
        ));

        assert_eq!(issue_ids(&app), ["b", "c"]);
        assert_eq!(app.current_tree_issue().unwrap().id, "b");
        assert!(app.row_is_expanded(&app.rows[1]));
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn refresh_keeps_path_specific_expansion_when_topology_is_unchanged() {
        let shared_graph = || {
            IssueGraph::new(
                vec![
                    Issue {
                        id: "a".into(),
                        dependencies: vec![Dependency {
                            id: "c".into(),
                            ..Dependency::default()
                        }],
                        ..Issue::default()
                    },
                    Issue {
                        id: "d".into(),
                        dependencies: vec![Dependency {
                            id: "c".into(),
                            ..Dependency::default()
                        }],
                        ..Issue::default()
                    },
                    Issue {
                        id: "c".into(),
                        dependencies: vec![Dependency {
                            id: "e".into(),
                            ..Dependency::default()
                        }],
                        ..Issue::default()
                    },
                    Issue {
                        id: "e".into(),
                        ..Issue::default()
                    },
                ],
                vec![],
            )
        };
        let mut app = App::new(shared_graph());
        app.handle_key(key(KeyCode::Char('l'))); // expand a
        app.handle_key(key(KeyCode::Char('j')));
        app.handle_key(key(KeyCode::Char('l'))); // expand c under a
        app.cursor = 4;
        app.handle_key(key(KeyCode::Char('l'))); // expand d, but not c under d

        app.refresh_graph(shared_graph());

        assert_eq!(issue_ids(&app), ["a", "c", "e", "d", "c"]);
        assert_eq!(app.current_tree_issue().unwrap().id, "d");
    }

    #[test]
    fn dependencies_outside_the_filtered_list_stay_out_of_the_tree() {
        let mut app = App::new(IssueGraph::new(
            vec![Issue {
                id: "open".into(),
                title: "Open".into(),
                dependencies: vec![Dependency {
                    id: "closed".into(),
                    title: "Closed".into(),
                    status: "closed".into(),
                    ..Dependency::default()
                }],
                ..Issue::default()
            }],
            vec![],
        ));

        app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(issue_ids(&app), ["open"]);

        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.selected_dependency().unwrap().id, "closed");
    }

    #[test]
    fn slash_fuzzy_filters_visible_ids_and_enter_opens_match() {
        let mut app = App::new(IssueGraph::new(
            vec![
                Issue {
                    id: "issue-alpha".into(),
                    ..Issue::default()
                },
                Issue {
                    id: "issue-jbeta".into(),
                    ..Issue::default()
                },
            ],
            vec![],
        ));

        app.handle_key(key(KeyCode::Char('/')));
        app.handle_key(key(KeyCode::Char('j')));
        app.handle_key(key(KeyCode::Char('b')));
        app.handle_key(key(KeyCode::Char('t')));
        assert_eq!(app.rows.len(), 1);
        assert_eq!(app.rows[0].issue_id, "issue-jbeta");

        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.search_query(), None);
        assert_eq!(app.current_detail_issue().unwrap().id, "issue-jbeta");
    }

    #[test]
    fn escape_cancels_search_and_restores_tree_selection() {
        let mut app = App::new(graph());
        app.handle_key(key(KeyCode::Char('/')));
        app.handle_key(key(KeyCode::Char('z')));
        assert!(app.rows.is_empty());

        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.search_query(), None);
        assert_eq!(app.current_tree_issue().unwrap().id, "a");
    }

    #[test]
    fn enter_on_the_create_entry_starts_a_top_level_add_issue() {
        let mut app = App::new(graph());
        app.handle_key(key(KeyCode::Char('g')));
        assert!(app.rows[app.cursor].is_create_entry());

        app.handle_key(key(KeyCode::Enter));

        let flow = app.add_issue_flow().unwrap();
        assert_eq!(flow.parent_id, None);
        assert_eq!(flow.step, AddIssueStep::Title);
    }

    #[test]
    fn create_entry_is_available_even_when_the_tree_is_empty() {
        let mut app = App::new(IssueGraph::new(vec![], vec![]));
        assert_eq!(app.rows.len(), 1);
        assert!(app.rows[0].is_create_entry());

        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.add_issue_flow().unwrap().parent_id, None);
    }

    #[test]
    fn create_entry_ignores_issue_keys_and_stays_out_of_search() {
        let mut app = App::new(graph());
        app.handle_key(key(KeyCode::Char('g')));

        assert_eq!(app.handle_key(key(KeyCode::Char('x'))), Action::None);
        assert!(!app.is_confirming_close());
        assert_eq!(app.handle_key(key(KeyCode::Char('e'))), Action::None);
        assert_eq!(app.flush_pending_key(), Action::None);
        app.handle_key(key(KeyCode::Char('s')));
        assert!(app.picker().is_none());

        app.handle_key(key(KeyCode::Char('/')));
        app.handle_key(key(KeyCode::Char('c')));
        assert!(app.rows.is_empty(), "'c' must not match the create entry");
    }

    #[test]
    fn s_opens_a_status_picker_preselecting_the_current_status() {
        let mut app = App::new(IssueGraph::new(
            vec![Issue {
                id: "a".into(),
                status: "in_progress".into(),
                ..Issue::default()
            }],
            vec![],
        ));

        app.handle_key(key(KeyCode::Char('s')));
        let picker = app.picker().unwrap();
        assert_eq!(picker.kind, PickerKind::Status);
        assert_eq!(picker.issue_id, "a");
        assert_eq!(STATUSES[picker.index], "in_progress");

        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Action::SetStatus("a".into(), "blocked")
        );
        assert!(app.picker().is_none());
        assert_eq!(app.screen(), Screen::Tree);
    }

    #[test]
    fn p_opens_a_priority_picker_in_the_task_view_too() {
        let mut app = App::new(graph());
        app.handle_key(key(KeyCode::Enter));

        app.handle_key(key(KeyCode::Char('p')));
        let picker = app.picker().unwrap();
        assert_eq!(picker.kind, PickerKind::Priority);
        assert_eq!(PRIORITIES[picker.index], 0);

        app.handle_key(key(KeyCode::Char('j')));
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Action::SetPriority("a".into(), 2)
        );
        assert_eq!(app.screen(), Screen::Detail, "picker returns to task view");
    }

    #[test]
    fn escape_cancels_a_picker_without_an_action() {
        let mut app = App::new(graph());
        app.handle_key(key(KeyCode::Char('s')));
        assert!(app.picker().is_some());

        assert_eq!(app.handle_key(key(KeyCode::Esc)), Action::None);
        assert!(app.picker().is_none());
        assert_eq!(app.screen(), Screen::Tree);
    }

    #[test]
    fn search_only_includes_rows_visible_before_search() {
        let mut app = App::new(graph());

        app.handle_key(key(KeyCode::Char('/')));
        app.handle_key(key(KeyCode::Char('b')));
        assert!(
            app.rows.is_empty(),
            "collapsed child must not be searchable"
        );

        app.handle_key(key(KeyCode::Esc));
        app.handle_key(key(KeyCode::Char('l')));
        app.handle_key(key(KeyCode::Char('/')));
        app.handle_key(key(KeyCode::Char('b')));
        assert_eq!(app.rows.len(), 1);
        assert_eq!(app.rows[0].issue_id, "b");
        assert_eq!(app.rows[0].prefix, "└── ");
    }
}

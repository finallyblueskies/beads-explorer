use crate::model::{Dependency, Issue, IssueGraph};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashSet;
use std::time::{Duration, Instant};

const EDIT_SEQUENCE_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    pub issue_id: String,
    pub path: Vec<String>,
    pub prefix: String,
    pub cycle: bool,
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
pub enum Action {
    None,
    Quit,
    EditDescription,
    EditTitle,
}

pub struct App {
    pub graph: IssueGraph,
    pub rows: Vec<TreeRow>,
    pub cursor: usize,
    pub scroll: usize,
    pub viewport: usize,
    search_query: Option<String>,
    search_origin_cursor: usize,
    search_origin_scroll: usize,
    tree_rows: Vec<TreeRow>,
    expanded: HashSet<Vec<String>>,
    history: Vec<DetailFrame>,
    edit_key_started: Option<Instant>,
}

impl App {
    pub fn new(graph: IssueGraph) -> Self {
        let mut app = Self {
            graph,
            rows: Vec::new(),
            cursor: 0,
            scroll: 0,
            viewport: 1,
            search_query: None,
            search_origin_cursor: 0,
            search_origin_scroll: 0,
            tree_rows: Vec::new(),
            expanded: HashSet::new(),
            history: Vec::new(),
            edit_key_started: None,
        };
        app.rebuild_rows();
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
        self.search_query.as_deref()
    }

    pub fn current_detail_issue(&self) -> Option<&Issue> {
        self.history
            .last()
            .and_then(|frame| self.graph.issue(&frame.issue_id))
    }

    pub fn detail_frame(&self) -> Option<&DetailFrame> {
        self.history.last()
    }

    pub fn detail_frame_mut(&mut self) -> Option<&mut DetailFrame> {
        self.history.last_mut()
    }

    pub fn row_is_expanded(&self, row: &TreeRow) -> bool {
        self.expanded.contains(&row.path)
    }

    pub fn row_has_children(&self, row: &TreeRow) -> bool {
        !row.cycle && !self.graph.tree_children(&row.issue_id).is_empty()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.edit_key_started = None;
            return Action::Quit;
        }
        match self.screen() {
            Screen::Tree => self.handle_tree_key(key),
            Screen::Detail => self.handle_detail_key(key),
        }
    }

    pub fn pending_key_timeout(&self) -> Option<Duration> {
        self.edit_key_started
            .map(|started| EDIT_SEQUENCE_TIMEOUT.saturating_sub(started.elapsed()))
    }

    pub fn flush_pending_key(&mut self) -> Action {
        if self.edit_key_started.take().is_some() {
            Action::EditDescription
        } else {
            Action::None
        }
    }

    fn handle_tree_key(&mut self, key: KeyEvent) -> Action {
        if self.search_query.is_some() {
            return self.handle_search_key(key);
        }

        if key.code == KeyCode::Char('/') {
            self.start_search();
            return Action::None;
        }

        if self.rows.is_empty() {
            return match key.code {
                KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
                _ => Action::None,
            };
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
            KeyCode::Enter => self.open_selected_issue(),
            KeyCode::Char('q') | KeyCode::Esc => return Action::Quit,
            _ => {}
        }
        Action::None
    }

    fn handle_search_key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.cancel_search(),
            KeyCode::Enter => self.open_search_result(),
            KeyCode::Char('d') if ctrl => self.move_cursor(self.viewport as isize / 2),
            KeyCode::Char('u') if ctrl => {
                if let Some(query) = self.search_query.as_mut() {
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
                if let Some(query) = self.search_query.as_mut() {
                    query.pop();
                }
                self.rebuild_search_rows();
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(query) = self.search_query.as_mut() {
                    query.push(character);
                }
                self.rebuild_search_rows();
            }
            _ => {}
        }
        Action::None
    }

    fn handle_detail_key(&mut self, key: KeyEvent) -> Action {
        if let Some(started) = self.edit_key_started.take() {
            let plain_t = key.code == KeyCode::Char('t')
                && !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
            return if plain_t && started.elapsed() < EDIT_SEQUENCE_TIMEOUT {
                Action::EditTitle
            } else {
                Action::EditDescription
            };
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
            KeyCode::Backspace => {
                self.history.pop();
            }
            KeyCode::Esc => self.history.clear(),
            KeyCode::Char('q') => return Action::Quit,
            _ => {}
        }
        Action::None
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
            self.history.push(DetailFrame {
                issue_id: row.issue_id.clone(),
                dependency_cursor: 0,
                scroll: 0,
            });
        }
    }

    fn start_search(&mut self) {
        self.search_origin_cursor = self.cursor;
        self.search_origin_scroll = self.scroll;
        self.search_query = Some(String::new());
        self.rebuild_search_rows();
    }

    fn cancel_search(&mut self) {
        self.search_query = None;
        self.rows.clone_from(&self.tree_rows);
        self.cursor = self
            .search_origin_cursor
            .min(self.rows.len().saturating_sub(1));
        self.scroll = self.search_origin_scroll;
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
        let Some(query) = self.search_query.as_deref() else {
            return;
        };
        self.rows = self
            .tree_rows
            .iter()
            .filter(|row| fuzzy_match(&row.issue_id, query))
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

    fn rebuild_rows(&mut self) {
        fn walk(
            graph: &IssueGraph,
            expanded: &HashSet<Vec<String>>,
            path: &[String],
            prefix: &str,
            rows: &mut Vec<TreeRow>,
        ) {
            if !expanded.contains(path) {
                return;
            }
            let Some(issue_id) = path.last() else {
                return;
            };
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
                        &child_path,
                        &format!("{prefix}{}", if last { "    " } else { "│   " }),
                        rows,
                    );
                }
            }
        }

        let selected_path = self.rows.get(self.cursor).map(|row| row.path.clone());
        let mut rows = Vec::new();
        for root in self.graph.roots() {
            let path = vec![root.clone()];
            rows.push(TreeRow {
                issue_id: root.clone(),
                path: path.clone(),
                prefix: String::new(),
                cycle: false,
            });
            walk(&self.graph, &self.expanded, &path, "", &mut rows);
        }
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

    #[test]
    fn tree_starts_at_first_level_and_expands_like_treec() {
        let mut app = App::new(graph());
        assert_eq!(app.rows.len(), 1);
        assert_eq!(app.rows[0].issue_id, "a");

        app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(app.rows.len(), 2);
        assert_eq!(app.rows[1].issue_id, "b");

        app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(app.cursor, 1);
        app.handle_key(key(KeyCode::Char('h')));
        assert_eq!(app.cursor, 0);
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

        assert_eq!(app.rows.len(), 1);
        assert_eq!(app.rows[0].issue_id, "8gda");

        app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(app.rows.len(), 2);
        assert_eq!(app.rows[1].issue_id, "hmb2");
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
    fn e_in_task_view_requests_description_edit_after_the_sequence_timeout() {
        let mut app = App::new(graph());
        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.handle_key(key(KeyCode::Char('e'))), Action::None);
        assert_eq!(app.flush_pending_key(), Action::EditDescription);
        assert_eq!(app.current_detail_issue().unwrap().id, "a");
    }

    #[test]
    fn e_then_t_in_task_view_requests_title_edit() {
        let mut app = App::new(graph());
        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.handle_key(key(KeyCode::Char('e'))), Action::None);
        assert_eq!(app.handle_key(key(KeyCode::Char('t'))), Action::EditTitle);
        assert_eq!(app.current_detail_issue().unwrap().id, "a");
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
        assert_eq!(app.rows.len(), 1);

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
        assert_eq!(app.rows[0].issue_id, "a");
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

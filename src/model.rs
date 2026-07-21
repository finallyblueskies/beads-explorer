use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Output};

// `bd show` hydrates dependencies into issue summaries (`id`, `title`, ...),
// while `bd list` emits raw edge records (`depends_on_id`, `type`, ...). The
// aliases let one shape cover both.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Dependency {
    #[serde(alias = "depends_on_id")]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default, alias = "type")]
    pub dependency_type: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Issue {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub issue_type: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditField {
    Title,
    Description,
}

/// Everything `bd create` needs for a new issue; `parent_id` is `None` for a
/// top-level issue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddIssueDraft {
    pub parent_id: Option<String>,
    pub title: String,
    pub description: String,
    pub issue_type: String,
    pub priority: i32,
}

#[derive(Clone, Debug, Default)]
pub struct IssueGraph {
    issues: HashMap<String, Issue>,
    order: Vec<String>,
    listed: HashSet<String>,
    tree_children: HashMap<String, Vec<String>>,
    roots: Vec<String>,
}

impl IssueGraph {
    /// `listed` issues form the tree; `context` issues (e.g. closed ones) only
    /// hydrate dependency targets reached through Task View.
    pub fn new(listed_issues: Vec<Issue>, context: Vec<Issue>) -> Self {
        let mut order = Vec::with_capacity(listed_issues.len());
        let mut map = HashMap::with_capacity(listed_issues.len() + context.len());
        for issue in listed_issues {
            if !map.contains_key(&issue.id) {
                order.push(issue.id.clone());
            }
            map.insert(issue.id.clone(), issue);
        }
        let listed: HashSet<String> = order.iter().cloned().collect();
        for issue in context {
            map.entry(issue.id.clone()).or_insert(issue);
        }

        // A dependency can point to a deleted or externally sourced issue. Keep a
        // navigable placeholder rather than silently dropping that graph edge.
        let missing: Vec<Issue> = map
            .values()
            .flat_map(|issue| issue.dependencies.iter())
            .filter(|dep| !map.contains_key(&dep.id))
            .map(|dep| Issue {
                id: dep.id.clone(),
                title: if dep.title.is_empty() {
                    "(not found)".to_string()
                } else {
                    dep.title.clone()
                },
                status: if dep.status.is_empty() {
                    "?".to_string()
                } else {
                    dep.status.clone()
                },
                priority: dep.priority,
                ..Issue::default()
            })
            .collect();
        for issue in missing {
            map.entry(issue.id.clone()).or_insert(issue);
        }

        // `bd` stores parent-child on the child as a dependency on its parent.
        // Reverse that relationship for the tree while leaving other dependency
        // types pointing from an issue to its prerequisite. Only listed issues
        // shape the tree, so edges to or from context issues are omitted.
        let mut tree_children: HashMap<String, Vec<String>> = HashMap::new();
        let mut targets = HashSet::new();
        for issue_id in &order {
            let Some(issue) = map.get(issue_id) else {
                continue;
            };
            for dependency in &issue.dependencies {
                if !listed.contains(&dependency.id) {
                    continue;
                }
                let (parent, child) = if dependency.dependency_type == "parent-child" {
                    (dependency.id.clone(), issue.id.clone())
                } else {
                    (issue.id.clone(), dependency.id.clone())
                };
                let children = tree_children.entry(parent).or_default();
                if !children.contains(&child) {
                    children.push(child.clone());
                }
                targets.insert(child);
            }
        }
        let mut roots: Vec<String> = order
            .iter()
            .filter(|id| !targets.contains(id.as_str()))
            .cloned()
            .collect();

        // A graph made solely of cycles has no mathematical root. Showing each
        // component entry is more useful than an empty explorer.
        if roots.is_empty() {
            roots.clone_from(&order);
        }

        Self {
            issues: map,
            order,
            listed,
            tree_children,
            roots,
        }
    }

    pub fn issue(&self, id: &str) -> Option<&Issue> {
        self.issues.get(id)
    }

    pub fn roots(&self) -> &[String] {
        &self.roots
    }

    pub fn is_listed(&self, id: &str) -> bool {
        self.listed.contains(id)
    }

    pub fn tree_children(&self, id: &str) -> &[String] {
        self.tree_children.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn replace_issue(&mut self, issue: Issue) {
        self.issues.insert(issue.id.clone(), issue);
    }

    /// Optimistic local mirror of `bd close`: the issue leaves the listed set
    /// and joins the context issues, so the UI can update before `bd` confirms.
    pub fn with_issue_closed(&self, issue_id: &str) -> IssueGraph {
        let listed_issues: Vec<Issue> = self
            .order
            .iter()
            .filter(|id| id.as_str() != issue_id)
            .filter_map(|id| self.issues.get(id))
            .cloned()
            .collect();
        let mut context: Vec<Issue> = self
            .issues
            .values()
            .filter(|issue| !self.listed.contains(&issue.id))
            .cloned()
            .collect();
        if let Some(issue) = self.issues.get(issue_id) {
            let mut issue = issue.clone();
            issue.status = "closed".to_string();
            context.push(issue);
        }
        IssueGraph::new(listed_issues, context)
    }
}

/// The `bd` CLI plus the `--db` selection, shared by every operation.
#[derive(Clone, Debug)]
pub struct Bd {
    program: OsString,
    db: Option<PathBuf>,
}

impl Bd {
    pub fn new(program: OsString, db: Option<PathBuf>) -> Self {
        Self { program, db }
    }

    fn name(&self) -> String {
        self.program.to_string_lossy().into_owned()
    }

    /// `--db` goes after `args` so subcommand positionals stay in front.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.program);
        command.args(args);
        if let Some(path) = &self.db {
            command.arg("--db").arg(path);
        }
        command
    }

    /// Runs to completion; a non-zero exit becomes an error built from stderr,
    /// or stdout when stderr is empty.
    fn run(&self, verb: &str, command: &mut Command) -> io::Result<Output> {
        let output = command.output().map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("could not run {}: {error}", self.name()),
            )
        })?;
        if output.status.success() {
            return Ok(output);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let message = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        Err(io::Error::other(if message.is_empty() {
            format!("{} {verb} failed: {}", self.name(), output.status)
        } else {
            format!("{} {verb} failed: {message}", self.name())
        }))
    }

    fn run_json(&self, args: &[&str]) -> io::Result<Value> {
        let verb = args.first().copied().unwrap_or("command");
        let output = self.run(verb, &mut self.command(args))?;
        serde_json::from_slice(&output.stdout).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} returned invalid JSON for {verb}: {error}", self.name()),
            )
        })
    }

    pub fn load(&self) -> io::Result<IssueGraph> {
        // One `bd list --all` call: per-ID `bd show` resolution costs ~250ms per
        // issue, so hydrating dependency targets from the same payload instead
        // keeps startup at a single fast query.
        let issues = parse_issue_collection(self.run_json(&["list", "--all", "--json"])?)?;
        let (listed, context): (Vec<Issue>, Vec<Issue>) = issues
            .into_iter()
            .partition(|issue| matches!(issue.status.as_str(), "open" | "in_progress"));
        if listed.is_empty() {
            return Ok(IssueGraph::default());
        }
        Ok(IssueGraph::new(listed, context))
    }

    pub fn load_issue(&self, issue_id: &str) -> io::Result<Issue> {
        parse_issue_collection(self.run_json(&["show", issue_id, "--json"])?)?
            .into_iter()
            .find(|issue| issue.id == issue_id)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{} show returned no issue {issue_id}", self.name()),
                )
            })
    }

    /// `bd edit` opens `$EDITOR` itself, so the command inherits the terminal
    /// instead of having its output captured.
    pub fn edit(&self, field: EditField, issue_id: &str) -> io::Result<()> {
        let flag = match field {
            EditField::Title => "--title",
            EditField::Description => "--description",
        };
        let status = self
            .command(&["edit", issue_id, flag])
            .status()
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("could not run {}: {error}", self.name()),
                )
            })?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "{} edit failed: {status}",
                self.name()
            )))
        }
    }

    pub fn close_issue(&self, issue_id: &str) -> io::Result<()> {
        self.run("close", &mut self.command(&["close", issue_id]))
            .map(|_| ())
    }

    pub fn set_status(&self, issue_id: &str, status: &str) -> io::Result<()> {
        self.run(
            "update",
            &mut self.command(&["update", issue_id, "--status", status]),
        )
        .map(|_| ())
    }

    pub fn set_priority(&self, issue_id: &str, priority: i32) -> io::Result<()> {
        self.run(
            "update",
            &mut self.command(&["update", issue_id, "--priority", &format!("P{priority}")]),
        )
        .map(|_| ())
    }

    pub fn create_issue(&self, draft: &AddIssueDraft) -> io::Result<String> {
        let priority = format!("P{}", draft.priority);
        let mut args = vec![
            "create",
            draft.title.as_str(),
            "--description",
            draft.description.as_str(),
            "--type",
            draft.issue_type.as_str(),
            "--priority",
            priority.as_str(),
        ];
        if let Some(parent_id) = &draft.parent_id {
            args.push("--parent");
            args.push(parent_id);
        }
        args.push("--silent");
        let output = self.run("create", &mut self.command(&args))?;
        let issue_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if issue_id.is_empty() {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} create returned no issue ID", self.name()),
            ))
        } else {
            Ok(issue_id)
        }
    }
}

fn parse_issue_collection(value: Value) -> io::Result<Vec<Issue>> {
    let issues = match value {
        Value::Array(values) => values,
        Value::Object(mut object) if object.contains_key("issues") => object
            .remove("issues")
            .and_then(|value| value.as_array().cloned())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "`issues` is not an array")
            })?,
        Value::Object(object) if object.contains_key("id") => vec![Value::Object(object)],
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected issue JSON shape from bd",
            ))
        }
    };

    issues
        .into_iter()
        .map(|value| {
            serde_json::from_value(value).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("could not decode issue JSON: {error}"),
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::time::{SystemTime, UNIX_EPOCH};

    fn issue(id: &str, deps: &[&str]) -> Issue {
        Issue {
            id: id.to_string(),
            title: id.to_string(),
            dependencies: deps
                .iter()
                .map(|id| Dependency {
                    id: (*id).to_string(),
                    ..Dependency::default()
                })
                .collect(),
            ..Issue::default()
        }
    }

    #[test]
    fn roots_are_issues_not_targeted_by_another_issue() {
        let graph = IssueGraph::new(
            vec![issue("a", &["b"]), issue("b", &[]), issue("c", &[])],
            vec![],
        );
        assert_eq!(graph.roots(), &["a", "c"]);
    }

    #[test]
    fn parent_child_edges_are_reversed_for_the_tree() {
        let mut child = issue("hmb2", &["8gda"]);
        child.dependencies[0].dependency_type = "parent-child".to_string();
        let graph = IssueGraph::new(vec![child, issue("8gda", &[])], vec![]);

        assert_eq!(graph.roots(), &["8gda"]);
        assert_eq!(graph.tree_children("8gda"), &["hmb2"]);
        assert!(graph.tree_children("hmb2").is_empty());
    }

    #[test]
    fn cycles_remain_visible() {
        let graph = IssueGraph::new(vec![issue("a", &["b"]), issue("b", &["a"])], vec![]);
        assert_eq!(graph.roots(), &["a", "b"]);
    }

    #[test]
    fn context_issues_hydrate_dependencies_without_joining_the_tree() {
        let mut closed = issue("z", &["a"]);
        closed.status = "closed".to_string();
        let graph = IssueGraph::new(vec![issue("a", &["z"])], vec![closed]);

        assert_eq!(graph.issue("z").unwrap().title, "z");
        assert_eq!(graph.issue("z").unwrap().status, "closed");
        assert!(!graph.is_listed("z"));
        assert_eq!(graph.len(), 1);
        assert_eq!(graph.roots(), &["a"]);
    }

    #[test]
    fn with_issue_closed_moves_the_issue_into_context() {
        let graph = IssueGraph::new(vec![issue("a", &["b"]), issue("b", &[])], vec![]);

        let closed = graph.with_issue_closed("a");

        assert_eq!(closed.roots(), &["b"]);
        assert!(!closed.is_listed("a"));
        assert!(closed.is_listed("b"));
        assert_eq!(closed.issue("a").unwrap().status, "closed");
    }

    #[test]
    fn replacing_issue_refreshes_its_description() {
        let mut graph = IssueGraph::new(vec![issue("a", &[])], vec![]);
        let mut refreshed = issue("a", &[]);
        refreshed.description = "Edited description".to_string();

        graph.replace_issue(refreshed);

        assert_eq!(graph.issue("a").unwrap().description, "Edited description");
        assert!(graph.is_listed("a"));
    }

    #[test]
    fn parses_list_and_show_shapes() {
        let wrapped = serde_json::json!({"issues": [{"id": "a", "title": "A"}]});
        let array = serde_json::json!([{"id": "b", "title": "B"}]);
        assert_eq!(parse_issue_collection(wrapped).unwrap()[0].id, "a");
        assert_eq!(parse_issue_collection(array).unwrap()[0].id, "b");
    }

    #[test]
    fn parses_hydrated_and_edge_dependency_shapes() {
        let value = serde_json::json!([{
            "id": "a",
            "dependencies": [
                {"id": "b", "title": "B", "status": "open", "priority": 1, "dependency_type": "blocks"},
                {"issue_id": "a", "depends_on_id": "c", "type": "parent-child", "metadata": "{}"},
            ],
        }]);
        let deps = parse_issue_collection(value).unwrap()[0]
            .dependencies
            .clone();
        assert_eq!(deps[0].id, "b");
        assert_eq!(deps[0].dependency_type, "blocks");
        assert_eq!(deps[1].id, "c");
        assert_eq!(deps[1].dependency_type, "parent-child");
    }

    #[cfg(unix)]
    #[test]
    fn load_lists_open_and_in_progress_issues_by_default() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let script =
            std::env::temp_dir().join(format!("be-fake-bd-load-{}-{nonce}", std::process::id()));
        fs::write(
            &script,
            "#!/bin/sh\nprintf '%s\\n' '[{\"id\":\"open\",\"status\":\"open\"},{\"id\":\"working\",\"status\":\"in_progress\"},{\"id\":\"closed\",\"status\":\"closed\"}]'\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&script, permissions).unwrap();

        let graph = Bd::new(script.clone().into_os_string(), None)
            .load()
            .unwrap();

        assert!(graph.is_listed("open"));
        assert!(graph.is_listed("working"));
        assert!(!graph.is_listed("closed"));
        assert_eq!(graph.len(), 2);

        let _ = fs::remove_file(script);
    }

    #[cfg(unix)]
    #[test]
    fn create_issue_passes_parent_type_and_p1_to_bd() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir();
        let script = directory.join(format!("be-fake-bd-{}-{nonce}", std::process::id()));
        let arguments = directory.join(format!("be-fake-bd-args-{}-{nonce}", std::process::id()));
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf 'child-1\\n'\n",
                arguments.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&script, permissions).unwrap();

        let bd = Bd::new(
            script.clone().into_os_string(),
            Some(PathBuf::from("/tmp/example.db")),
        );
        let issue_id = bd
            .create_issue(&AddIssueDraft {
                parent_id: Some("parent-1".into()),
                title: "Child title".into(),
                description: "Body text".into(),
                issue_type: "feature".into(),
                priority: 1,
            })
            .unwrap();

        assert_eq!(issue_id, "child-1");
        assert_eq!(
            fs::read_to_string(&arguments)
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            vec![
                "create",
                "Child title",
                "--description",
                "Body text",
                "--type",
                "feature",
                "--priority",
                "P1",
                "--parent",
                "parent-1",
                "--silent",
                "--db",
                "/tmp/example.db",
            ]
        );

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(arguments);
    }

    #[cfg(unix)]
    #[test]
    fn create_issue_without_parent_omits_the_parent_flag() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir();
        let script = directory.join(format!("be-fake-bd-root-{}-{nonce}", std::process::id()));
        let arguments = directory.join(format!(
            "be-fake-bd-root-args-{}-{nonce}",
            std::process::id()
        ));
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf 'root-1\\n'\n",
                arguments.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&script, permissions).unwrap();

        let issue_id = Bd::new(script.clone().into_os_string(), None)
            .create_issue(&AddIssueDraft {
                parent_id: None,
                title: "Top level".into(),
                description: String::new(),
                issue_type: "task".into(),
                priority: 1,
            })
            .unwrap();

        assert_eq!(issue_id, "root-1");
        let recorded = fs::read_to_string(&arguments).unwrap();
        assert!(!recorded.lines().any(|argument| argument == "--parent"));
        assert!(recorded.lines().any(|argument| argument == "--silent"));

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(arguments);
    }

    #[cfg(unix)]
    #[test]
    fn set_status_and_priority_run_bd_update() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir();
        let script = directory.join(format!("be-fake-bd-update-{}-{nonce}", std::process::id()));
        let arguments = directory.join(format!(
            "be-fake-bd-update-args-{}-{nonce}",
            std::process::id()
        ));
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\n",
                arguments.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&script, permissions).unwrap();

        let bd = Bd::new(script.clone().into_os_string(), None);
        bd.set_status("task-1", "in_progress").unwrap();
        bd.set_priority("task-1", 2).unwrap();

        assert_eq!(
            fs::read_to_string(&arguments)
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            vec![
                "update",
                "task-1",
                "--status",
                "in_progress",
                "update",
                "task-1",
                "--priority",
                "P2",
            ]
        );

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(arguments);
    }
}

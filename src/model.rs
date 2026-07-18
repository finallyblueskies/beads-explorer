use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::process::Command;

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
}

pub fn load(bd: &OsStr, db: Option<&Path>) -> io::Result<IssueGraph> {
    // One `bd list --all` call: per-ID `bd show` resolution costs ~250ms per
    // issue, so hydrating dependency targets from the same payload instead
    // keeps startup at a single fast query.
    let mut args = vec!["list", "--all", "--json"];
    let db_string = db.map(|path| path.to_string_lossy().into_owned());
    if let Some(path) = db_string.as_deref() {
        args.extend(["--db", path]);
    }
    let issues = parse_issue_collection(run_bd_json(bd, &args)?)?;
    let (listed, context): (Vec<Issue>, Vec<Issue>) =
        issues.into_iter().partition(|issue| issue.status == "open");
    if listed.is_empty() {
        return Ok(IssueGraph::default());
    }
    Ok(IssueGraph::new(listed, context))
}

pub fn edit_description(bd: &OsStr, db: Option<&Path>, issue_id: &str) -> io::Result<()> {
    let mut command = Command::new(bd);
    command.args(["edit", issue_id, "--description"]);
    if let Some(path) = db {
        command.arg("--db").arg(path);
    }

    let status = command.status().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not run {}: {error}", bd.to_string_lossy()),
        )
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{} edit failed: {status}",
            bd.to_string_lossy()
        )))
    }
}

pub fn load_issue(bd: &OsStr, db: Option<&Path>, issue_id: &str) -> io::Result<Issue> {
    let db_string = db.map(|path| path.to_string_lossy().into_owned());
    let mut args = vec!["show", issue_id, "--json"];
    if let Some(path) = db_string.as_deref() {
        args.extend(["--db", path]);
    }
    parse_issue_collection(run_bd_json(bd, &args)?)?
        .into_iter()
        .find(|issue| issue.id == issue_id)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} show returned no issue {issue_id}", bd.to_string_lossy()),
            )
        })
}

fn run_bd_json(bd: &OsStr, args: &[&str]) -> io::Result<Value> {
    let output = Command::new(bd).args(args).output().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not run {}: {error}", bd.to_string_lossy()),
        )
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let message = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        return Err(io::Error::other(format!(
            "{} {} failed: {}",
            bd.to_string_lossy(),
            args.first().copied().unwrap_or(""),
            if message.is_empty() {
                output.status.to_string()
            } else {
                message.to_string()
            }
        )));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} returned invalid JSON for {}: {error}",
                bd.to_string_lossy(),
                args.first().copied().unwrap_or("command")
            ),
        )
    })
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
}

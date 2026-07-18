use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::process::Command;

const SHOW_CHUNK_SIZE: usize = 128;

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
    roots: Vec<String>,
}

impl IssueGraph {
    pub fn new(issues: Vec<Issue>) -> Self {
        let mut order = Vec::with_capacity(issues.len());
        let mut map = HashMap::with_capacity(issues.len());
        for issue in issues {
            if !map.contains_key(&issue.id) {
                order.push(issue.id.clone());
            }
            map.insert(issue.id.clone(), issue);
        }
        let listed = order.iter().cloned().collect();

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

        let targets: HashSet<&str> = map
            .values()
            .flat_map(|issue| issue.dependencies.iter().map(|dep| dep.id.as_str()))
            .collect();
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
    let mut list_args = vec!["list", "--status", "open", "--json"];
    let db_string = db.map(|path| path.to_string_lossy().into_owned());
    if let Some(path) = db_string.as_deref() {
        list_args.extend(["--db", path]);
    }
    let list_value = run_bd_json(bd, &list_args)?;
    let summaries = parse_issue_collection(list_value)?;
    if summaries.is_empty() {
        return Ok(IssueGraph::default());
    }

    let mut summary_map: HashMap<String, Issue> = summaries
        .iter()
        .cloned()
        .map(|issue| (issue.id.clone(), issue))
        .collect();
    let mut detailed = Vec::with_capacity(summaries.len());

    for chunk in summaries.chunks(SHOW_CHUNK_SIZE) {
        let mut args = Vec::with_capacity(chunk.len() + 5);
        args.push("show");
        args.extend(chunk.iter().map(|issue| issue.id.as_str()));
        args.push("--json");
        if let Some(path) = db_string.as_deref() {
            args.extend(["--db", path]);
        }
        detailed.extend(parse_issue_collection(run_bd_json(bd, &args)?)?);
    }

    for issue in &mut detailed {
        if let Some(summary) = summary_map.remove(&issue.id) {
            fill_missing_fields(issue, summary);
        }
    }
    detailed.extend(summary_map.into_values());

    // Restore the stable ordering returned by `bd list` after merging chunks.
    let positions: HashMap<&str, usize> = summaries
        .iter()
        .enumerate()
        .map(|(index, issue)| (issue.id.as_str(), index))
        .collect();
    detailed.sort_by_key(|issue| {
        positions
            .get(issue.id.as_str())
            .copied()
            .unwrap_or(usize::MAX)
    });

    Ok(IssueGraph::new(detailed))
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

fn fill_missing_fields(issue: &mut Issue, summary: Issue) {
    if issue.title.is_empty() {
        issue.title = summary.title;
    }
    if issue.description.is_empty() {
        issue.description = summary.description;
    }
    if issue.status.is_empty() {
        issue.status = summary.status;
    }
    if issue.issue_type.is_empty() {
        issue.issue_type = summary.issue_type;
    }
    if issue.created_at.is_empty() {
        issue.created_at = summary.created_at;
    }
    if issue.updated_at.is_empty() {
        issue.updated_at = summary.updated_at;
    }
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
        let graph = IssueGraph::new(vec![issue("a", &["b"]), issue("b", &[]), issue("c", &[])]);
        assert_eq!(graph.roots(), &["a", "c"]);
    }

    #[test]
    fn cycles_remain_visible() {
        let graph = IssueGraph::new(vec![issue("a", &["b"]), issue("b", &["a"])]);
        assert_eq!(graph.roots(), &["a", "b"]);
    }

    #[test]
    fn replacing_issue_refreshes_its_description() {
        let mut graph = IssueGraph::new(vec![issue("a", &[])]);
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

use beads_explorer::app::{Action, App};
use beads_explorer::model::{Bd, IssueGraph};
use beads_explorer::ui;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use std::env;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How often the event loop wakes to check on a background close.
const CLOSE_POLL_INTERVAL: Duration = Duration::from_millis(50);

const HELP: &str = "beads explorer

Usage: be [OPTIONS]

Options:
      --db <PATH>   Use a specific beads database
      --bd <PATH>   Path to the bd executable [env: BEADS_EXPLORER_BD]
  -h, --help        Print help
  -V, --version     Print version

Tree:      j/k move · h/l fold · Tab toggle · Enter open (creates on the + Create New entry)
           / go to · q/Esc quit
Task view: j/k dependency · Enter open · Backspace back · Esc tree · q quit
Both:      e edit description · et edit title · s set status · p set priority
           + add child · x close issue (with confirmation)

Add issue: Enter advances · j/k selects type/priority · e opens $EDITOR for text
           Backspace returns to the previous selection · Esc asks before discarding
";

enum CloseOutcome {
    Reloaded(IssueGraph),
    CloseFailed(io::Error),
    ReloadFailed(io::Error),
}

/// A `bd close` running on a background thread. `rollback` restores the
/// pre-close graph if `bd` rejects the close after the optimistic update.
struct PendingClose {
    outcome: mpsc::Receiver<CloseOutcome>,
    rollback: IssueGraph,
}

impl PendingClose {
    fn spawn(bd: &Bd, issue_id: String, rollback: IssueGraph) -> Self {
        let (sender, receiver) = mpsc::channel();
        let bd = bd.clone();
        thread::spawn(move || {
            let outcome = match bd.close_issue(&issue_id) {
                Err(error) => CloseOutcome::CloseFailed(error),
                Ok(()) => match bd.load() {
                    Ok(graph) => CloseOutcome::Reloaded(graph),
                    Err(error) => CloseOutcome::ReloadFailed(error),
                },
            };
            let _ = sender.send(outcome);
        });
        Self {
            outcome: receiver,
            rollback,
        }
    }

    /// Applies a finished close to `app`; returns itself while still running.
    fn poll(self, app: &mut App) -> Option<Self> {
        match self.outcome.try_recv() {
            Ok(CloseOutcome::Reloaded(graph)) => {
                app.refresh_graph(graph);
                app.clear_status();
            }
            Ok(CloseOutcome::CloseFailed(error)) => {
                app.refresh_graph(self.rollback);
                app.set_status(error.to_string());
            }
            Ok(CloseOutcome::ReloadFailed(error)) => {
                app.set_status(format!("{error} · view may be stale"));
            }
            Err(mpsc::TryRecvError::Empty) => return Some(self),
            Err(mpsc::TryRecvError::Disconnected) => {
                app.refresh_graph(self.rollback);
                app.set_status("close failed: background worker died".to_string());
            }
        }
        None
    }
}

struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    fn activate(&mut self, out: &mut impl Write) -> io::Result<()> {
        if !self.active {
            terminal::enable_raw_mode()?;
            self.active = true;
            execute!(out, EnterAlternateScreen, Hide)?;
        }
        Ok(())
    }

    fn restore(&mut self, out: &mut impl Write) -> io::Result<()> {
        if self.active {
            execute!(out, LeaveAlternateScreen, Show)?;
            terminal::disable_raw_mode()?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let _ = terminal::disable_raw_mode();
        if let Ok(mut tty) = OpenOptions::new().write(true).open("/dev/tty") {
            let _ = execute!(tty, LeaveAlternateScreen, Show);
        } else {
            let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
        }
    }
}

fn parse_args() -> Result<Option<Bd>, String> {
    let mut args = env::args_os().skip(1);
    let mut bd = env::var_os("BEADS_EXPLORER_BD").unwrap_or_else(|| OsString::from("bd"));
    let mut db = None;
    while let Some(argument) = args.next() {
        match argument.to_string_lossy().as_ref() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("be {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--bd" => {
                bd = args
                    .next()
                    .ok_or_else(|| "--bd requires a path".to_string())?;
            }
            "--db" => {
                db = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--db requires a path".to_string())?,
                ));
            }
            unknown => return Err(format!("unknown option: {unknown}\n\n{HELP}")),
        }
    }
    Ok(Some(Bd::new(bd, db)))
}

fn edit_add_issue_field(initial: &str) -> io::Result<String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = env::temp_dir().join(format!("be-add-{}-{nonce}.md", std::process::id()));
    fs::write(&path, initial)?;

    let result = (|| {
        let status = Command::new("sh")
            .args(["-c", "exec ${VISUAL:-${EDITOR:-vi}} \"$1\"", "be-editor"])
            .arg(&path)
            .status()
            .map_err(|error| {
                io::Error::new(error.kind(), format!("could not open editor: {error}"))
            })?;
        if !status.success() {
            return Err(io::Error::other(format!("editor failed: {status}")));
        }
        fs::read_to_string(&path)
    })();
    let _ = fs::remove_file(&path);
    result
}

fn read_key_action(app: &mut App) -> io::Result<Action> {
    Ok(match event::read()? {
        Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
        _ => Action::None,
    })
}

/// Blocks for the next key unless a pending `e` chord or background close
/// needs the loop to wake up on a timeout.
fn next_action(app: &mut App, close_pending: bool) -> io::Result<Action> {
    let key_timeout = app.pending_key_timeout();
    let timeout = if close_pending {
        Some(key_timeout.map_or(CLOSE_POLL_INTERVAL, |timeout| {
            timeout.min(CLOSE_POLL_INTERVAL)
        }))
    } else {
        key_timeout
    };
    let Some(timeout) = timeout else {
        return read_key_action(app);
    };
    if event::poll(timeout)? {
        read_key_action(app)
    } else if app
        .pending_key_timeout()
        .is_some_and(|remaining| remaining.is_zero())
    {
        Ok(app.flush_pending_key())
    } else {
        Ok(Action::None)
    }
}

/// Applies a finished `bd update`, reloading the whole graph on success since
/// a status change can move an issue in or out of the tree.
fn apply_update(app: &mut App, bd: &Bd, result: io::Result<()>, success: String) {
    match result {
        Ok(()) => match bd.load() {
            Ok(graph) => {
                app.refresh_graph(graph);
                app.set_status(success);
            }
            Err(error) => app.set_status(format!("{error} · view may be stale")),
        },
        Err(error) => app.set_status(error.to_string()),
    }
}

fn run(bd: Bd) -> io::Result<()> {
    let mut app = App::new(bd.load()?);

    let output: Box<dyn Write> = match OpenOptions::new().read(true).write(true).open("/dev/tty") {
        Ok(tty) => Box::new(tty),
        Err(_) => Box::new(io::stdout()),
    };
    let mut out = BufWriter::new(output);
    let mut guard = TerminalGuard { active: false };
    guard.activate(&mut out)?;

    let mut pending_close: Option<PendingClose> = None;

    loop {
        pending_close = pending_close.and_then(|pending| pending.poll(&mut app));

        let (width, height) = terminal::size()?;
        ui::draw(&mut app, &mut out, width, height)?;

        match next_action(&mut app, pending_close.is_some())? {
            Action::Quit => break,
            Action::CloseIssue(_)
            | Action::Edit(_)
            | Action::EditAddIssue(_)
            | Action::CreateIssue(_)
            | Action::SetStatus(..)
            | Action::SetPriority(..)
                if pending_close.is_some() =>
            {
                app.set_status("waiting for the previous close to finish".to_string());
            }
            Action::CloseIssue(issue_id) => {
                let rollback = app.graph.clone();
                let optimistic = app.graph.with_issue_closed(&issue_id);
                app.return_to_tree();
                app.refresh_graph(optimistic);
                app.set_status(format!("Closing {issue_id}…"));
                pending_close = Some(PendingClose::spawn(&bd, issue_id, rollback));
            }
            Action::Edit(field) => {
                let Some(issue_id) = app.current_issue().map(|issue| issue.id.clone()) else {
                    continue;
                };
                guard.restore(&mut out)?;
                let edit_result = bd
                    .edit(field, &issue_id)
                    .and_then(|_| bd.load_issue(&issue_id));
                guard.activate(&mut out)?;
                match edit_result {
                    Ok(issue) => app.graph.replace_issue(issue),
                    Err(error) => app.set_status(error.to_string()),
                }
            }
            Action::EditAddIssue(field) => {
                let initial = app.add_issue_field(field).unwrap_or("").to_string();
                guard.restore(&mut out)?;
                let edit_result = edit_add_issue_field(&initial);
                guard.activate(&mut out)?;
                match edit_result {
                    Ok(value) => app.set_add_issue_field(field, value),
                    Err(error) => app.set_status(error.to_string()),
                }
            }
            Action::CreateIssue(draft) => match bd.create_issue(&draft) {
                Ok(issue_id) => {
                    app.finish_add_issue();
                    let created = match &draft.parent_id {
                        Some(parent_id) => format!("Created {issue_id} under {parent_id}"),
                        None => format!("Created {issue_id}"),
                    };
                    match bd.load() {
                        Ok(graph) => {
                            app.refresh_graph(graph);
                            app.set_status(created);
                        }
                        Err(error) => app.set_status(format!(
                            "Created {issue_id}, but reload failed: {error} · view may be stale"
                        )),
                    }
                }
                Err(error) => app.set_status(error.to_string()),
            },
            Action::SetStatus(issue_id, status) => {
                let result = bd.set_status(&issue_id, status);
                apply_update(&mut app, &bd, result, format!("Set {issue_id} to {status}"));
            }
            Action::SetPriority(issue_id, priority) => {
                let result = bd.set_priority(&issue_id, priority);
                apply_update(
                    &mut app,
                    &bd,
                    result,
                    format!("Set {issue_id} to P{priority}"),
                );
            }
            Action::None => {}
        }
    }

    guard.restore(&mut out)
}

fn main() -> ExitCode {
    let bd = match parse_args() {
        Ok(Some(bd)) => bd,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("be: {message}");
            return ExitCode::from(2);
        }
    };

    match run(bd) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("be: {error}");
            ExitCode::FAILURE
        }
    }
}

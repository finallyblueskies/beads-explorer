use beads_explorer::app::{Action, App};
use beads_explorer::{model, ui};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use std::env;
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// How often the event loop wakes to check on a background close.
const CLOSE_POLL_INTERVAL: Duration = Duration::from_millis(50);

const HELP: &str = "beads explorer

Usage: be [OPTIONS]

Options:
      --db <PATH>   Use a specific beads database
      --bd <PATH>   Path to the bd executable [env: BEADS_EXPLORER_BD]
  -h, --help        Print help
  -V, --version     Print version

Tree:      j/k move · h/l fold · Tab toggle · Enter open · x close · q/Esc quit
Task view: j/k dependency · Enter open · e edit description · Backspace back · Esc tree
           et edit title · x close issue (with confirmation)
";

struct Options {
    bd: OsString,
    db: Option<PathBuf>,
}

enum CloseOutcome {
    Reloaded(model::IssueGraph),
    CloseFailed(io::Error),
    ReloadFailed(io::Error),
}

/// A `bd close` running on a background thread. `rollback` restores the
/// pre-close graph if `bd` rejects the close after the optimistic update.
struct PendingClose {
    outcome: mpsc::Receiver<CloseOutcome>,
    rollback: model::IssueGraph,
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

fn parse_args() -> Result<Option<Options>, String> {
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
    Ok(Some(Options { bd, db }))
}

fn run(options: Options) -> io::Result<()> {
    let graph = model::load(&options.bd, options.db.as_deref())?;
    let mut app = App::new(graph);

    let output: Box<dyn Write> = match OpenOptions::new().read(true).write(true).open("/dev/tty") {
        Ok(tty) => Box::new(tty),
        Err(_) => Box::new(io::stdout()),
    };
    let mut out = BufWriter::new(output);
    terminal::enable_raw_mode()?;
    let mut guard = TerminalGuard { active: true };
    execute!(out, EnterAlternateScreen, Hide)?;

    let mut pending_close: Option<PendingClose> = None;

    loop {
        if let Some(pending) = pending_close.take() {
            match pending.outcome.try_recv() {
                Ok(CloseOutcome::Reloaded(graph)) => {
                    app.refresh_graph(graph);
                    app.clear_status();
                }
                Ok(CloseOutcome::CloseFailed(error)) => {
                    app.refresh_graph(pending.rollback);
                    app.set_status(error.to_string());
                }
                Ok(CloseOutcome::ReloadFailed(error)) => {
                    app.set_status(format!("{error} · view may be stale"));
                }
                Err(mpsc::TryRecvError::Empty) => pending_close = Some(pending),
                Err(mpsc::TryRecvError::Disconnected) => {
                    app.refresh_graph(pending.rollback);
                    app.set_status("close failed: background worker died".to_string());
                }
            }
        }

        let (width, height) = terminal::size()?;
        ui::draw(&mut app, &mut out, width, height)?;

        let key_timeout = app.pending_key_timeout();
        let poll_timeout = if pending_close.is_some() {
            Some(key_timeout.map_or(CLOSE_POLL_INTERVAL, |timeout| {
                timeout.min(CLOSE_POLL_INTERVAL)
            }))
        } else {
            key_timeout
        };
        let action = if let Some(timeout) = poll_timeout {
            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
                    _ => Action::None,
                }
            } else if app
                .pending_key_timeout()
                .is_some_and(|remaining| remaining.is_zero())
            {
                app.flush_pending_key()
            } else {
                Action::None
            }
        } else {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
                _ => Action::None,
            }
        };

        match action {
            Action::Quit => break,
            Action::CloseIssue(_) | Action::EditDescription | Action::EditTitle
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

                let (sender, receiver) = mpsc::channel();
                let bd = options.bd.clone();
                let db = options.db.clone();
                thread::spawn(move || {
                    let outcome = match model::close_issue(&bd, db.as_deref(), &issue_id) {
                        Err(error) => CloseOutcome::CloseFailed(error),
                        Ok(()) => match model::load(&bd, db.as_deref()) {
                            Ok(graph) => CloseOutcome::Reloaded(graph),
                            Err(error) => CloseOutcome::ReloadFailed(error),
                        },
                    };
                    let _ = sender.send(outcome);
                });
                pending_close = Some(PendingClose {
                    outcome: receiver,
                    rollback,
                });
            }
            action @ (Action::EditDescription | Action::EditTitle) => {
                let Some(issue_id) = app.current_detail_issue().map(|issue| issue.id.clone())
                else {
                    continue;
                };
                guard.restore(&mut out)?;
                let edit_result = match action {
                    Action::EditDescription => {
                        model::edit_description(&options.bd, options.db.as_deref(), &issue_id)
                    }
                    Action::EditTitle => {
                        model::edit_title(&options.bd, options.db.as_deref(), &issue_id)
                    }
                    _ => unreachable!(),
                }
                .and_then(|_| model::load_issue(&options.bd, options.db.as_deref(), &issue_id));
                guard.activate(&mut out)?;
                match edit_result {
                    Ok(issue) => app.graph.replace_issue(issue),
                    Err(error) => app.set_status(error.to_string()),
                }
            }
            Action::None => {}
        }
    }

    guard.restore(&mut out)
}

fn main() -> ExitCode {
    let options = match parse_args() {
        Ok(Some(options)) => options,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("be: {message}");
            return ExitCode::from(2);
        }
    };

    match run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("be: {error}");
            ExitCode::FAILURE
        }
    }
}

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

const HELP: &str = "beads explorer

Usage: be [OPTIONS]

Options:
      --db <PATH>   Use a specific beads database
      --bd <PATH>   Path to the bd executable [env: BEADS_EXPLORER_BD]
  -h, --help        Print help
  -V, --version     Print version

Tree:      j/k move · h/l fold · Tab toggle · Enter open · q/Esc quit
Task view: j/k dependency · Enter open · e edit description · Backspace back · Esc tree
           et edit title
";

struct Options {
    bd: OsString,
    db: Option<PathBuf>,
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

    loop {
        let (width, height) = terminal::size()?;
        ui::draw(&mut app, &mut out, width, height)?;
        let action = if let Some(timeout) = app.pending_key_timeout() {
            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
                    _ => Action::None,
                }
            } else {
                app.flush_pending_key()
            }
        } else {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
                _ => Action::None,
            }
        };

        match action {
            Action::Quit => break,
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
                app.graph.replace_issue(edit_result?);
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

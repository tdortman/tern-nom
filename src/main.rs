//! `nom`, drawn natively when it runs in a Tern pane.
//!
//! `nom build|copy|flake …` and `nom --json` (how `nh` pipes nix into it)
//! become a live Tern card; everything else, and every run outside Tern,
//! is handed to the real nix-output-monitor named by `TERN_NOM_REAL`.

mod drv;
mod history;
mod nix;
mod state;
mod tty;
mod view;

use std::{
    io::{self, BufRead as _, BufReader, Read, Write as _},
    os::unix::process::CommandExt as _,
    process::{Command, ExitCode, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use state::{Outcome, State};
use tern_sdk::{Input, Options, Session, SurfaceOptions};
use view::Msg;

/// How often the card is redrawn while output arrives.
const FRAME: Duration = Duration::from_millis(100);

/// How long one wait for terminal input lasts.
const POLL: Duration = Duration::from_millis(30);

/// How often running builds' timers and estimates are redrawn.
const TICK: Duration = Duration::from_secs(1);

/// Set by SIGINT/SIGTERM.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

/// Where nix's log comes from.
enum Source {
    /// `nix <args>` run by this process.
    Spawn(Vec<String>),
    /// A pipe on stdin (`nom --json`).
    Stdin,
}

/// Replaces this process with the real nom.
fn real_nom(args: &[String]) -> ExitCode {
    let Some(real) = std::env::var_os("TERN_NOM_REAL") else {
        eprintln!("tern-nom: TERN_NOM_REAL is not set; it names the real nix-output-monitor's nom");
        return ExitCode::FAILURE;
    };
    let err = Command::new(real).arg0("nom").args(args).exec();
    eprintln!("tern-nom: cannot run the real nom: {err}");
    ExitCode::FAILURE
}

/// A Tern session on the controlling terminal, when there is one.
fn connect() -> Option<Session<Msg>> {
    let set = |name| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    // Multiplexers swallow the handshake; outside a Tern pane it would only
    // cost the handshake timeout.
    // shortcut: no TSP over ssh (TERN_PANE isn't forwarded), revisit if builds run remotely.
    if std::env::var_os("TERN_TSP").is_some_and(|v| v == "0")
        || set("TMUX")
        || set("STY")
        || set("ZELLIJ")
        || !(set("TERN_PANE") || std::env::var_os("TERM_PROGRAM").is_some_and(|v| v == "tern"))
    {
        return None;
    }
    let tty = tty::DevTty::open().ok()?;
    let options = Options::new()
        .app("nom")
        .version(env!("CARGO_PKG_VERSION"))
        .bracketed_paste(false)
        .kitty_keyboard(false)
        .timeout(Duration::from_millis(500));
    Session::with_terminal(tty, options).ok().flatten()
}

/// The command line of this process's parent (`nh os switch`), for the
/// card's title under a pipe.
fn parent_command() -> Option<String> {
    // SAFETY: getppid has no preconditions.
    let ppid = unsafe { libc::getppid() };
    let raw = std::fs::read(format!("/proc/{ppid}/cmdline")).ok()?;
    let args: Vec<_> = raw
        .split(|&b| b == 0)
        .filter(|a| !a.is_empty())
        .map(String::from_utf8_lossy)
        .collect();
    let program = args
        .first()?
        .rsplit('/')
        .next()?
        .trim_start_matches('.')
        .trim_end_matches("-wrapped");
    let mut title = program.to_owned();
    for arg in &args[1..] {
        title.push(' ');
        title.push_str(arg);
    }
    Some(title)
}

/// Streams `input` line by line into `tx`.
fn pump_lines(input: impl Read + Send + 'static, tx: mpsc::Sender<nix::Line>) {
    thread::spawn(move || {
        let mut input = BufReader::new(input);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match input.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf);
                    let line = line.trim_end_matches(['\n', '\r']);
                    if tx.send(nix::Line::parse(line)).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

/// Monitors `source` on `session`; the process's exit code.
fn monitor(
    mut session: Session<Msg>,
    source: Source,
    title: String,
) -> Result<ExitCode, tern_sdk::Error> {
    let (tx, rx) = mpsc::channel();
    let mut child = None;
    let mut stdout = None;
    match source {
        Source::Spawn(args) => {
            let mut cmd = Command::new("nix");
            let (sub, rest) = args.split_first().expect("a subcommand");
            cmd.arg(sub)
                .args(["-v", "--log-format", "internal-json"])
                .args(rest);
            let mut spawned = match cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
                Ok(c) => c,
                Err(e) => {
                    session.close()?;
                    eprintln!("tern-nom: cannot run nix: {e}");
                    return Ok(ExitCode::FAILURE);
                }
            };
            pump_lines(spawned.stderr.take().expect("piped stderr"), tx);
            let mut out = spawned.stdout.take().expect("piped stdout");
            // Printed after the card closes, as nom does (`--print-out-paths`).
            stdout = Some(thread::spawn(move || {
                let mut bytes = Vec::new();
                let _ = out.read_to_end(&mut bytes);
                bytes
            }));
            child = Some(spawned);
        }
        Source::Stdin => pump_lines(io::stdin(), tx),
    }

    let sf = session.open(SurfaceOptions::flow().title(title.clone()))?;
    let reader: state::DrvReader = Box::new(|path| std::fs::read_to_string(path).ok());
    let mut state = State::new(history::History::load(), reader);
    // The `.drv` whose log the user opened by clicking its row.
    let mut open: Option<String> = None;
    session.render(sf, view::view(&state, &title, None))?;
    let mut last = Instant::now();
    let mut dirty = false;
    loop {
        if INTERRUPTED.load(Ordering::Relaxed) {
            state.finish(Outcome::Cancelled);
            break;
        }
        // A click on a running build opens or closes its log; reading also
        // lets acks free frames.
        if let Some(Input::Msg(Msg::ToggleLog(drv), _)) = session.next(Some(POLL))? {
            open = if open.as_deref() == Some(drv.as_str()) {
                None
            } else {
                Some(drv)
            };
            session.render(sf, view::view(&state, &title, open.as_deref()))?;
            last = Instant::now();
        }
        let ended = loop {
            // What nix logs while it dies (builds stopping) is not progress.
            if INTERRUPTED.load(Ordering::Relaxed) {
                break false;
            }
            match rx.try_recv() {
                Ok(line) => {
                    state.apply(line);
                    dirty = true;
                }
                Err(mpsc::TryRecvError::Empty) => break false,
                Err(mpsc::TryRecvError::Disconnected) => break true,
            }
        };
        if ended {
            break;
        }
        // Timers and estimate bars are text, and rows appear after a second,
        // so redraw while builds or copies run.
        let tick = !(state.builds.is_empty() && state.copies.is_empty()) && last.elapsed() >= TICK;
        if (dirty && last.elapsed() >= FRAME) || tick {
            session.render(sf, view::view(&state, &title, open.as_deref()))?;
            last = Instant::now();
            dirty = false;
        }
    }

    // A Ctrl+C reached nix (same process group) too, so this wait is short.
    let status = child.as_mut().map(std::process::Child::wait).transpose()?;
    if state.outcome.is_none() {
        let ok = status.is_none_or(|s| s.success());
        state.finish(if ok { Outcome::Done } else { Outcome::Failed });
    }
    session.render(sf, view::view(&state, &title, open.as_deref()))?;
    session.close()?;
    if let Err(e) = state.history.save() {
        eprintln!("tern-nom: cannot save build times to nom's history: {e}");
    }

    if let Some(bytes) = stdout.and_then(|h| h.join().ok()) {
        let mut out = io::stdout().lock();
        let _ = out.write_all(&bytes);
        let _ = out.flush();
    }
    Ok(match (state.outcome.map(|o| o.0), status) {
        (Some(Outcome::Cancelled), _) => ExitCode::from(130),
        (_, Some(s)) => ExitCode::from(s.code().and_then(|c| u8::try_from(c).ok()).unwrap_or(1)),
        (Some(Outcome::Failed), None) => ExitCode::FAILURE,
        _ => ExitCode::SUCCESS,
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let source = match args.first().map(String::as_str) {
        Some("build" | "copy" | "flake") if !args.iter().any(|a| a == "--help") => {
            Source::Spawn(args.clone())
        }
        Some("--json") if args.len() == 1 => Source::Stdin,
        _ => return real_nom(&args),
    };
    let Some(session) = connect() else {
        return real_nom(&args);
    };
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
    }
    let title = match &source {
        Source::Spawn(a) => format!("nom {}", a.join(" ")),
        Source::Stdin => parent_command().unwrap_or_else(|| "nix".to_owned()),
    };
    match monitor(session, source, title) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("tern-nom: {e}");
            ExitCode::FAILURE
        }
    }
}

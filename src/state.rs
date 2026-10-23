//! What a nix invocation is doing, folded from its log messages.

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    time::{Duration, Instant},
};

use crate::nix::{
    Line, Message, act, drv_paths, num_field, res, store_name, str_field, strip_ansi,
};

/// Build log lines kept for the log view.
const LOG_LINES: usize = 400;

/// A derivation being built.
#[derive(Debug)]
pub struct Build {
    /// Its `.drv` path.
    pub drv: String,
    /// Its package name.
    pub name: String,
    /// The remote builder, if not local.
    pub machine: Option<String>,
    /// The current stdenv phase.
    pub phase: Option<String>,
    /// The last non-empty line it printed, without escapes.
    pub last_line: Option<String>,
}

/// A store path being copied or substituted.
#[derive(Debug)]
pub struct Copy {
    /// Its store path.
    pub path: String,
    /// Its package name.
    pub name: String,
    /// Where it comes from.
    pub from: Option<String>,
    /// Bytes copied.
    pub done: u64,
    /// Bytes to copy (unpacked NAR size), 0 while unknown.
    pub expected: u64,
}

/// A build or copy that ended.
#[derive(Debug)]
pub struct Finished {
    /// Its `.drv` (builds) or store path (copies).
    pub path: String,
    /// Package name.
    pub name: String,
    /// How long it ran.
    pub took: Duration,
    /// Whether nix reported it failed.
    pub failed: bool,
    /// Bytes copied (copies only).
    pub bytes: u64,
}

/// How serious a message is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// `error: …`.
    Error,
    /// `warning: …` and level-1 messages.
    Warning,
    /// Traces and notices.
    Info,
}

/// A message nix printed for the user.
#[derive(Debug)]
pub struct Note {
    /// How serious it is.
    pub severity: Severity,
    /// The message, with ANSI colors.
    pub text: String,
}

/// `[done, expected, running, failed]` of a counting activity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Finished successfully.
    pub done: u64,
    /// In total.
    pub expected: u64,
    /// Running now.
    pub running: u64,
    /// Failed.
    pub failed: u64,
}

impl Counts {
    /// From a `Progress` result's fields.
    fn from_fields(fields: &[serde_json::Value]) -> Self {
        Self {
            done: num_field(fields, 0),
            expected: num_field(fields, 1),
            running: num_field(fields, 2),
            failed: num_field(fields, 3),
        }
    }

    /// Field-wise sum.
    fn add(self, other: Self) -> Self {
        Self {
            done: self.done + other.done,
            expected: self.expected + other.expected,
            running: self.running + other.running,
            failed: self.failed + other.failed,
        }
    }
}

/// How the invocation ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Everything succeeded.
    Done,
    /// Something failed.
    Failed,
    /// The user interrupted it.
    Cancelled,
}

/// Everything known about the invocation so far.
#[derive(Debug)]
pub struct State {
    /// The type of every running activity, by id.
    kinds: HashMap<u64, u32>,
    /// When each running build or copy started, by id.
    starts: HashMap<u64, Instant>,
    /// Builds running now, in start order.
    pub builds: BTreeMap<u64, Build>,
    /// Copies running now, in start order.
    pub copies: BTreeMap<u64, Copy>,
    /// Build counts per `Builds` activity.
    build_counts: HashMap<u64, Counts>,
    /// Path counts per `CopyPaths` activity.
    copy_counts: HashMap<u64, Counts>,
    /// Expected copy bytes per `Realise` activity.
    expected_bytes: HashMap<u64, u64>,
    /// Builds that ended, in end order.
    pub built: Vec<Finished>,
    /// Copies that ended, in end order.
    pub copied: Vec<Finished>,
    /// Bytes of the copies that ended.
    copied_bytes: u64,
    /// Derivations nix reported as failed.
    failed_drvs: HashSet<String>,
    /// Errors, warnings and traces.
    pub notes: Vec<Note>,
    /// The latest build output, `name> line`.
    pub log: VecDeque<String>,
    /// What nix does outside builds and copies (evaluation, queries).
    pub activity: Option<String>,
    /// The id of the activity `activity` describes.
    activity_id: u64,
    /// When monitoring started.
    pub started: Instant,
    /// How long it took and how it ended, once it ended.
    pub outcome: Option<(Outcome, Duration)>,
}

impl State {
    /// An invocation that starts now.
    pub fn new() -> Self {
        Self {
            kinds: HashMap::new(),
            starts: HashMap::new(),
            builds: BTreeMap::new(),
            copies: BTreeMap::new(),
            build_counts: HashMap::new(),
            copy_counts: HashMap::new(),
            expected_bytes: HashMap::new(),
            built: Vec::new(),
            copied: Vec::new(),
            copied_bytes: 0,
            failed_drvs: HashSet::new(),
            notes: Vec::new(),
            log: VecDeque::new(),
            activity: None,
            activity_id: 0,
            started: Instant::now(),
            outcome: None,
        }
    }

    /// Folds in one line of nix's stderr.
    pub fn apply(&mut self, line: Line) {
        match line {
            Line::Nix(msg) => self.message(msg),
            Line::Text(text) => {
                if !text.trim().is_empty() {
                    self.push_log(text);
                }
            }
        }
    }

    /// Folds in one structured message.
    fn message(&mut self, msg: Message) {
        match msg {
            Message::Start {
                id,
                text,
                kind,
                fields,
                ..
            } => self.start(id, kind, &text, &fields),
            Message::Stop { id } => self.stop(id),
            Message::Result { id, kind, fields } => self.result(id, kind, &fields),
            Message::Msg { level, msg } => self.note(level, msg),
            Message::Unknown => {}
        }
    }

    /// An activity started.
    fn start(&mut self, id: u64, kind: u32, text: &str, fields: &[serde_json::Value]) {
        self.kinds.insert(id, kind);
        match kind {
            act::BUILD => {
                let drv = str_field(fields, 0).unwrap_or_default().to_owned();
                let machine = str_field(fields, 1)
                    .filter(|m| !m.is_empty())
                    .map(str::to_owned);
                let name = store_name(&drv).to_owned();
                self.starts.insert(id, Instant::now());
                self.builds.insert(
                    id,
                    Build {
                        drv,
                        name,
                        machine,
                        phase: None,
                        last_line: None,
                    },
                );
            }
            act::COPY_PATH => {
                let path = str_field(fields, 0).unwrap_or_default().to_owned();
                let from = str_field(fields, 1)
                    .filter(|f| !f.is_empty())
                    .map(str::to_owned);
                self.starts.insert(id, Instant::now());
                let name = store_name(&path).to_owned();
                self.copies.insert(
                    id,
                    Copy {
                        path,
                        name,
                        from,
                        done: 0,
                        expected: 0,
                    },
                );
            }
            act::BUILDS => {
                self.build_counts.insert(id, Counts::default());
            }
            act::COPY_PATHS => {
                self.copy_counts.insert(id, Counts::default());
            }
            _ if !text.is_empty() => {
                self.activity = Some(text.to_owned());
                self.activity_id = id;
            }
            _ => {}
        }
    }

    /// An activity stopped.
    fn stop(&mut self, id: u64) {
        self.kinds.remove(&id);
        if self.activity_id == id {
            self.activity = None;
        }
        let took = self
            .starts
            .remove(&id)
            .map_or(Duration::ZERO, |t| t.elapsed());
        if let Some(build) = self.builds.remove(&id) {
            let failed = self.failed_drvs.contains(&build.drv);
            self.built.push(Finished {
                path: build.drv,
                name: build.name,
                took,
                failed,
                bytes: 0,
            });
        } else if let Some(copy) = self.copies.remove(&id) {
            let bytes = copy.done.max(copy.expected);
            self.copied_bytes += bytes;
            self.copied.push(Finished {
                path: copy.path,
                name: copy.name,
                took,
                failed: false,
                bytes,
            });
        }
    }

    /// A result for a running activity.
    fn result(&mut self, id: u64, kind: u32, fields: &[serde_json::Value]) {
        match kind {
            res::BUILD_LOG_LINE | res::POST_BUILD_LOG_LINE => {
                let Some(line) = str_field(fields, 0) else {
                    return;
                };
                if let Some(build) = self.builds.get_mut(&id) {
                    let plain = strip_ansi(line);
                    let plain = plain.trim();
                    if !plain.is_empty() {
                        build.last_line = Some(plain.to_owned());
                    }
                    let entry = format!("\x1b[2m{}>\x1b[0m {line}", build.name);
                    self.push_log(entry);
                } else {
                    self.push_log(line.to_owned());
                }
            }
            res::SET_PHASE => {
                if let (Some(build), Some(phase)) = (self.builds.get_mut(&id), str_field(fields, 0))
                {
                    build.phase = Some(phase.to_owned());
                }
            }
            res::PROGRESS => match self.kinds.get(&id).copied() {
                Some(act::BUILDS) => {
                    self.build_counts.insert(id, Counts::from_fields(fields));
                }
                Some(act::COPY_PATHS) => {
                    self.copy_counts.insert(id, Counts::from_fields(fields));
                }
                Some(act::COPY_PATH) => {
                    if let Some(copy) = self.copies.get_mut(&id) {
                        copy.done = num_field(fields, 0);
                        copy.expected = num_field(fields, 1);
                    }
                }
                _ => {}
            },
            res::SET_EXPECTED
                if self.kinds.get(&id) == Some(&act::REALISE)
                    && num_field(fields, 0) == u64::from(act::COPY_PATH) =>
            {
                self.expected_bytes.insert(id, num_field(fields, 1));
            }
            _ => {}
        }
    }

    /// A log message.
    fn note(&mut self, level: u8, text: String) {
        // Levels above notice are nix's own progress chatter under `-v`.
        if level > 2 {
            return;
        }
        let plain = strip_ansi(&text);
        let head = plain.trim_start();
        let severity = if level == 0 && head.starts_with("error") {
            Severity::Error
        } else if level == 1
            || head.starts_with("warning")
            || head.starts_with("evaluation warning")
        {
            Severity::Warning
        } else {
            Severity::Info
        };
        if severity == Severity::Error {
            self.mark_failed(&plain);
        }
        self.notes.push(Note { severity, text });
    }

    /// Marks builds an error names as failed; Lix reports a builder's failure
    /// after its activity stopped, nix while it still runs.
    fn mark_failed(&mut self, error: &str) {
        for drv in drv_paths(error) {
            if self.builds.values().any(|b| b.drv == drv) {
                self.failed_drvs.insert(drv.to_owned());
            } else if let Some(done) = self.built.iter_mut().find(|f| f.path == drv) {
                done.failed = true;
            }
        }
    }

    /// Appends to the log, dropping the oldest line when full.
    fn push_log(&mut self, line: String) {
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }

    /// Build counts: nix's own when it reports them, else what was seen.
    pub fn build_counts(&self) -> Counts {
        if self.build_counts.is_empty() {
            let failed = self.built.iter().filter(|b| b.failed).count() as u64;
            let done = self.built.len() as u64 - failed;
            let running = self.builds.len() as u64;
            return Counts {
                done,
                expected: done + failed + running,
                running,
                failed,
            };
        }
        self.build_counts
            .values()
            .fold(Counts::default(), |a, &b| a.add(b))
    }

    /// Path-copy counts: nix's own when it reports them, else what was seen.
    pub fn copy_counts(&self) -> Counts {
        if self.copy_counts.is_empty() {
            let done = self.copied.len() as u64;
            let running = self.copies.len() as u64;
            return Counts {
                done,
                expected: done + running,
                running,
                failed: 0,
            };
        }
        self.copy_counts
            .values()
            .fold(Counts::default(), |a, &b| a.add(b))
    }

    /// Bytes copied and bytes to copy in total.
    pub fn copy_bytes(&self) -> (u64, u64) {
        let running: u64 = self.copies.values().map(|c| c.done).sum();
        let left: u64 = self
            .copies
            .values()
            .map(|c| c.expected.saturating_sub(c.done))
            .sum();
        let done = self.copied_bytes + running;
        let announced: u64 = self.expected_bytes.values().sum();
        (done, announced.max(done + left))
    }

    /// Builds that failed.
    pub fn failed_builds(&self) -> usize {
        self.built.iter().filter(|b| b.failed).count()
    }

    /// Error messages.
    pub fn errors(&self) -> usize {
        self.notes
            .iter()
            .filter(|n| n.severity == Severity::Error)
            .count()
    }

    /// How long ago it started.
    pub fn started_ago(&self, id: u64) -> Duration {
        self.starts
            .get(&id)
            .map_or(Duration::ZERO, Instant::elapsed)
    }

    /// Ends the invocation; `success` is what the caller knows (nix's exit
    /// status), combined with failures seen in the log.
    pub fn finish(&mut self, outcome: Outcome) {
        let outcome = match outcome {
            Outcome::Done if self.failed_builds() + self.errors() > 0 => Outcome::Failed,
            other => other,
        };
        self.outcome = Some((outcome, self.started.elapsed()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds a captured log.
    fn feed(log: &str) -> State {
        let mut state = State::new();
        for line in log.lines() {
            state.apply(Line::parse(line));
        }
        state
    }

    /// `nix-build -v --log-format internal-json` (Lix 2.96) of four
    /// derivations and one substitution.
    const SUCCESS: &str = include_str!("../tests/lix-success.log");

    /// The same where one builder exits 3, failing its dependent.
    const FAILURE: &str = include_str!("../tests/lix-failure.log");

    #[test]
    fn successful_build_counts_builds_copies_and_bytes() {
        let mut state = feed(SUCCESS);
        state.finish(Outcome::Done);
        assert_eq!(
            state.build_counts(),
            Counts {
                done: 4,
                expected: 4,
                running: 0,
                failed: 0
            }
        );
        assert_eq!(state.copy_counts().done, 1);
        assert_eq!(state.copy_bytes(), (279_592, 279_592));
        assert!(state.builds.is_empty() && state.copies.is_empty());
        let mut names: Vec<_> = state.built.iter().map(|b| b.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["a-18439", "b-18439", "c-18439", "top-18439"]);
        assert_eq!(state.copied[0].name, "hello-2.12.3");
        assert!(
            state
                .log
                .iter()
                .any(|l| l.ends_with("top-18439>\x1b[0m top step 3"))
        );
        assert_eq!(state.errors(), 0);
        assert_eq!(state.outcome.map(|o| o.0), Some(Outcome::Done));
    }

    #[test]
    fn failed_builder_is_marked_and_fails_the_outcome() {
        let mut state = feed(FAILURE);
        state.finish(Outcome::Done);
        let failed: Vec<_> = state
            .built
            .iter()
            .filter(|b| b.failed)
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(failed, ["c-5863"]);
        assert_eq!(state.errors(), 2);
        assert_eq!(state.outcome.map(|o| o.0), Some(Outcome::Failed));
    }
}

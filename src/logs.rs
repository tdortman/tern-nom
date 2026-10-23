//! Every build's whole log, in files Tern opens and searches in a file block.

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write as _},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::nix::{store_name, strip_ansi};

/// Runs whose logs are kept, the newest first.
const RUNS: usize = 10;

/// One build's log file.
struct LogFile {
    path: PathBuf,
    /// Open while the build prints; reopened to append if it prints again.
    out: Option<BufWriter<File>>,
    lines: usize,
}

/// This run's log files, by `.drv` path.
pub struct Logs {
    dir: PathBuf,
    files: HashMap<String, LogFile>,
}

impl Logs {
    /// A fresh directory for this run under `$XDG_CACHE_HOME/tern-nom/logs`;
    /// older runs beyond the newest few are deleted.
    pub fn create() -> Option<Self> {
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cache")))?;
        let root = cache.join("tern-nom").join("logs");
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        let dir = root.join(format!("{secs:010}-{}", std::process::id()));
        fs::create_dir_all(&dir).ok()?;
        // Names start with the zero-padded start time, so they sort by age.
        let mut runs: Vec<PathBuf> = fs::read_dir(&root)
            .ok()?
            .filter_map(|e| Some(e.ok()?.path()))
            .collect();
        runs.sort();
        for old in runs.iter().rev().skip(RUNS) {
            let _ = fs::remove_dir_all(old);
        }
        Some(Self {
            dir,
            files: HashMap::new(),
        })
    }

    /// Appends `line` to `drv`'s log, without colors. A log that cannot be
    /// written stops growing; the card's own tail still shows it.
    pub fn write(&mut self, drv: &str, line: &str) {
        if !self.files.contains_key(drv) {
            let name = store_name(drv);
            let mut path = self.dir.join(format!("{name}.log"));
            // Many derivations share a name (`source`, `hook`).
            if self.files.values().any(|f| f.path == path) {
                let hash = drv.rsplit('/').next().unwrap_or(drv);
                path = self
                    .dir
                    .join(format!("{name}-{}.log", hash.get(..7).unwrap_or(hash)));
            }
            self.files.insert(
                drv.to_owned(),
                LogFile {
                    path,
                    out: None,
                    lines: 0,
                },
            );
        }
        let Some(file) = self.files.get_mut(drv) else {
            return;
        };
        if file.out.is_none() {
            file.out = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file.path)
                .ok()
                .map(BufWriter::new);
        }
        if let Some(out) = &mut file.out {
            if writeln!(out, "{}", strip_ansi(line)).is_err() {
                file.out = None;
                return;
            }
            file.lines += 1;
        }
    }

    /// Makes what was written readable, so an open file block shows it.
    pub fn flush(&mut self) {
        for file in self.files.values_mut() {
            if let Some(out) = &mut file.out {
                let _ = out.flush();
            }
        }
    }

    /// Closes `drv`'s log, so a long run does not hold a file per build.
    pub fn close(&mut self, drv: &str) {
        if let Some(file) = self.files.get_mut(drv) {
            file.out = None;
        }
    }

    /// `drv`'s log file and its line count, once it printed.
    pub fn get(&self, drv: &str) -> Option<(&Path, usize)> {
        self.files.get(drv).map(|f| (f.path.as_path(), f.lines))
    }
}

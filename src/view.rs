//! The state as a Tern card.

use std::time::Duration;

use tern_sdk::{
    nodes,
    ui::{self, Bound, Extent, Node, Status, View},
};

use crate::state::{Counts, Outcome, Severity, State};

/// Running builds or copies listed before the rest fold into "+N more".
const ROWS: usize = 8;

/// Build log lines sent to the window.
const LOG_TAIL: usize = 60;

/// `bytes` as MiB/GiB.
fn size(bytes: u64) -> String {
    let mib = bytes as f64 / (1024.0 * 1024.0);
    if mib >= 1024.0 {
        format!("{:.2} GiB", mib / 1024.0)
    } else {
        format!("{mib:.1} MiB")
    }
}

/// `d` as `1m 05s` / `4.2s`.
fn took(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{:.1}s", d.as_secs_f64())
    }
}

/// Milliseconds of `d`, as a node prop.
fn ms(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

/// `done/expected label` while running, `done label` once finished (nix
/// expects builds it then finds already valid), plus the failed count.
fn count_badges(c: Counts, label: &str, key: &str, finished: bool) -> Vec<Node> {
    let mut out = Vec::new();
    if (c.expected == 0 || finished) && c.done == 0 && c.failed == 0 {
        return out;
    }
    let tone = if c.failed > 0 {
        "error"
    } else if finished || c.done == c.expected {
        "success"
    } else {
        "neutral"
    };
    let text = if finished {
        format!("{} {label}", c.done)
    } else {
        format!("{}/{} {label}", c.done, c.expected)
    };
    out.push(ui::badge(text).tone(tone).key(format!("{key}.n")).into());
    if c.failed > 0 {
        out.push(
            ui::badge(format!("{} failed", c.failed))
                .tone("error")
                .key(format!("{key}.f"))
                .into(),
        );
    }
    out
}

/// The header row: activity, counts, timer.
fn summary(state: &State) -> Node {
    let mut row: Vec<Node> = Vec::new();
    match state.outcome {
        None => {
            let label = state.activity.as_deref().unwrap_or(
                if state.builds.is_empty() && state.copies.is_empty() {
                    "Evaluating"
                } else {
                    "Building"
                },
            );
            row.push(
                ui::spinner()
                    .style("dots")
                    .label(label)
                    .key("act")
                    .shrink(1.0)
                    .into(),
            );
        }
        Some((outcome, _)) => {
            let (icon, tone, label) = match outcome {
                Outcome::Done => ("check", "success", "Finished"),
                Outcome::Failed => ("x", "error", "Failed"),
                Outcome::Cancelled => ("stop", "warning", "Cancelled"),
            };
            row.push(ui::icon(icon).tone(tone).key("act").into());
            row.push(
                ui::text(ui::span(label).style("strong"))
                    .key("label")
                    .into(),
            );
        }
    }
    let finished = state.outcome.is_some();
    row.extend(count_badges(state.build_counts(), "built", "b", finished));
    row.extend(count_badges(state.copy_counts(), "fetched", "c", finished));
    let (done, total) = state.copy_bytes();
    if total > 0 {
        row.push(
            ui::text(ui::span(format!("↓ {} / {}", size(done), size(total))).style("muted"))
                .key("bytes")
                .into(),
        );
    }
    let warnings = state
        .notes
        .iter()
        .filter(|n| n.severity == Severity::Warning)
        .count();
    if warnings > 0 {
        let label = if warnings == 1 {
            "1 warning".to_owned()
        } else {
            format!("{warnings} warnings")
        };
        row.push(ui::badge(label).tone("warning").key("warn").into());
    }
    row.push(ui::spacer().grow(1.0).key("sp").into());
    let mut timer = ui::elapsed().age(ms(state.started.elapsed()));
    if let Some((_, d)) = state.outcome {
        timer = timer.stopped(u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
    }
    row.push(timer.key("t").into());
    ui::row()
        .gap("sm")
        .align("center")
        .wrap(true)
        .key("sum")
        .children(row)
        .into()
}

/// One row per running build: name, phase, last line, timer.
fn running_builds(state: &State) -> Option<Node> {
    if state.builds.is_empty() {
        return None;
    }
    let mut rows: Vec<Node> = Vec::new();
    for (&id, b) in state.builds.iter().take(ROWS) {
        // Still running when the invocation ended: interrupted.
        let indicator: Node = if state.outcome.is_some() {
            ui::icon("stop").tone("warning").key("s").into()
        } else {
            ui::spinner()
                .style("braille")
                .tone("accent")
                .key("s")
                .into()
        };
        let mut row = nodes![
            indicator,
            ui::text(ui::span(&b.name).style("strong"))
                .key("n")
                .shrink(0.0),
        ];
        if let Some(phase) = &b.phase {
            row.push(ui::badge(phase).tone("info").key("p").into());
        }
        if let Some(machine) = &b.machine {
            row.push(ui::badge(machine).tone("muted").key("m").into());
        }
        let line = b.last_line.as_deref().unwrap_or("");
        row.push(
            ui::text(ui::span(line).style("muted"))
                .truncate("end")
                .wrap("none")
                .grow(1.0)
                .shrink(1.0)
                .key("l")
                .into(),
        );
        let mut timer = ui::elapsed().age(ms(state.started_ago(id)));
        if state.outcome.is_some() {
            timer =
                timer.stopped(u64::try_from(state.started_ago(id).as_millis()).unwrap_or(u64::MAX));
        }
        row.push(timer.key("t").into());
        rows.push(
            ui::row()
                .gap("sm")
                .align("center")
                .key(id.to_string())
                .children(row)
                .into(),
        );
    }
    if state.builds.len() > ROWS {
        rows.push(
            ui::text(ui::span(format!("+{} more", state.builds.len() - ROWS)).style("muted"))
                .key("more")
                .into(),
        );
    }
    Some(ui::col().key("builds").children(rows).into())
}

/// One row per running copy with its own progress bar.
fn running_copies(state: &State) -> Option<Node> {
    if state.copies.is_empty() {
        return None;
    }
    let mut rows: Vec<Node> = Vec::new();
    for (&id, c) in state.copies.iter().take(ROWS) {
        let mut bar = ui::progress().tone("info").grow(1.0).key("b");
        if c.expected > 0 {
            bar = bar.value(c.done as f64 / c.expected as f64).label(format!(
                "{} / {}",
                size(c.done),
                size(c.expected)
            ));
        }
        let from = c
            .from
            .as_deref()
            .map_or("", |f| f.trim_start_matches("https://"));
        rows.push(
            ui::row()
                .gap("sm")
                .align("center")
                .key(id.to_string())
                .children(nodes![
                    ui::icon("download").tone("info").key("i"),
                    ui::text(ui::span(&c.name).style("strong"))
                        .key("n")
                        .shrink(0.0),
                    ui::text(ui::span(from).style("muted"))
                        .truncate("start")
                        .wrap("none")
                        .shrink(1.0)
                        .key("f"),
                    bar,
                ])
                .into(),
        );
    }
    if state.copies.len() > ROWS {
        rows.push(
            ui::text(ui::span(format!("+{} more", state.copies.len() - ROWS)).style("muted"))
                .key("more")
                .into(),
        );
    }
    Some(ui::col().key("copies").children(rows).into())
}

/// The overall build progress bar, while running.
fn progress(state: &State) -> Option<Node> {
    if state.outcome.is_some() {
        return None;
    }
    let b = state.build_counts();
    let c = state.copy_counts();
    let expected = b.expected + c.expected;
    if expected == 0 {
        return None;
    }
    let done = b.done + b.failed + c.done;
    Some(
        ui::progress()
            .value(done as f64 / expected as f64)
            .key("bar")
            .into(),
    )
}

/// Errors and warnings, as nix printed them.
fn notes(state: &State) -> Vec<Node> {
    state
        .notes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.severity != Severity::Info)
        .map(|(i, n)| {
            let tone = if n.severity == Severity::Error {
                "error"
            } else {
                "warning"
            };
            ui::ansi(n.text.replace('\n', "\r\n"))
                .tone(tone)
                .role("nom.note")
                .key(format!("n{i}"))
                .into()
        })
        .collect()
}

/// Builds that ended (failed first), in a section folded once all is well.
fn finished(state: &State) -> Option<Node> {
    if state.built.is_empty() && state.copied.is_empty() {
        return None;
    }
    let failed = state.failed_builds();
    let mut rows: Vec<Node> = Vec::new();
    let mut built: Vec<_> = state.built.iter().enumerate().collect();
    built.sort_by_key(|(_, b)| !b.failed);
    for (i, b) in built {
        let (icon, tone) = if b.failed {
            ("x", "error")
        } else {
            ("check", "success")
        };
        rows.push(
            ui::row()
                .gap("sm")
                .key(format!("b{i}"))
                .children(nodes![
                    ui::icon(icon).tone(tone).key("i"),
                    ui::text(ui::span(&b.name).style(if b.failed { "error" } else { "strong" }))
                        .key("n")
                        .grow(1.0),
                    ui::text(ui::span(took(b.took)).style("muted")).key("t"),
                ])
                .into(),
        );
    }
    for (i, c) in state.copied.iter().enumerate() {
        rows.push(
            ui::row()
                .gap("sm")
                .key(format!("c{i}"))
                .children(nodes![
                    ui::icon("download").tone("info").key("i"),
                    ui::text(&c.name).key("n").grow(1.0),
                    ui::text(ui::span(size(c.bytes)).style("muted")).key("s"),
                ])
                .into(),
        );
    }
    let head = format!(
        "Done · {} built, {} fetched",
        state.built.len() - failed,
        state.copied.len()
    );
    Some(
        ui::section()
            .head(head)
            .collapsible(true)
            .collapsed(failed == 0)
            .key("done")
            .max(Bound::h(Extent::Lines(20.0)))
            .children(rows)
            .into(),
    )
}

/// The tail of the build output.
fn log(state: &State) -> Option<Node> {
    if state.log.is_empty() {
        return None;
    }
    let skip = state.log.len().saturating_sub(LOG_TAIL);
    let mut text = String::new();
    for line in state.log.iter().skip(skip) {
        text.push_str(line);
        text.push_str("\x1b[0m\r\n");
    }
    let failed = matches!(state.outcome, Some((Outcome::Failed, _)));
    let collapsed = state.outcome.is_some() && !failed;
    Some(
        ui::section()
            .head("Build log")
            .collapsible(true)
            .collapsed(collapsed)
            .key("log")
            .child(
                ui::ansi(text)
                    .follow(true)
                    .max(Bound::h(Extent::Lines(12.0)))
                    .key("out"),
            )
            .into(),
    )
}

/// The whole card.
pub fn view(state: &State, title: &str) -> View {
    let status = match state.outcome {
        None => Status::Running,
        Some((Outcome::Done, _)) => Status::Done,
        Some((Outcome::Failed, _)) => Status::Error,
        Some((Outcome::Cancelled, _)) => Status::Cancelled,
    };
    let mut body: Vec<Node> = vec![summary(state)];
    body.extend(progress(state));
    body.extend(running_builds(state));
    body.extend(running_copies(state));
    body.extend(notes(state));
    body.extend(finished(state));
    body.extend(log(state));
    View::new().main([ui::card()
        .head(ui::span(title).style("mono"))
        .status(status)
        .role("nom.card")
        .key("nom")
        .children(body)])
}

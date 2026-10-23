//! The state as a Tern card.

use std::time::Duration;

use tern_sdk::{
    nodes,
    ui::{self, Bound, Extent, Status, TreeItem},
};

use crate::{
    nix::{store_name, strip_ansi},
    state::{Counts, DepNode, DrvStatus, Outcome, Severity, State},
};

/// What a click on the card asks for.
#[derive(Clone, Debug)]
pub enum Msg {
    /// Shows or hides this `.drv`'s log.
    ToggleLog(String),
}

type Node = ui::Node<Msg>;
type View = ui::View<Msg>;

/// Running builds or copies listed before the rest fold into "+N more".
const ROWS: usize = 8;

/// Build log lines sent to the window.
const LOG_TAIL: usize = 60;

/// Lines of a clicked build's log shown before "N more lines" expands it.
const PREVIEW: u32 = 8;

/// Lines of a clicked build's log sent, so expanding shows a longer tail.
const OPEN_TAIL: usize = 200;

/// `bytes` as KiB/MiB/GiB.
fn size(bytes: u64) -> String {
    let kib = bytes as f64 / 1024.0;
    if kib < 1024.0 {
        format!("{kib:.0} KiB")
    } else if kib < 1024.0 * 1024.0 {
        format!("{:.1} MiB", kib / 1024.0)
    } else {
        format!("{:.2} GiB", kib / (1024.0 * 1024.0))
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
    if !finished || c.done > 0 {
        out.push(ui::badge(text).tone(tone).key(format!("{key}.n")).into());
    }
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
            let label = state.activity.as_deref().map_or_else(
                || {
                    if !state.builds.is_empty() {
                        "Building"
                    } else if !state.copies.is_empty() {
                        "Fetching"
                    } else {
                        "Evaluating"
                    }
                    .to_owned()
                },
                shorten,
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
    // While running, the download bar below says this.
    let (fetched_bytes, _) = state.copy_bytes();
    if finished && fetched_bytes > 0 {
        row.push(
            ui::text(ui::span(format!("↓ {}", size(fetched_bytes))).style("muted"))
                .key("bytes")
                .into(),
        );
    }
    let warnings: usize = state
        .notes
        .iter()
        .filter(|n| n.severity == Severity::Warning)
        .map(|n| n.count)
        .sum();
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

/// One row per running build: name, phase, last line, timer. A click shows
/// the build's log under its row.
fn running_builds(state: &State, open: Option<&str>) -> Option<Node> {
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
        // nom's estimate from earlier builds of the same package.
        if let Some(est) = b.estimate {
            let elapsed = state.started_ago(id);
            let label = match est.checked_sub(elapsed) {
                Some(left) => format!("~{} left", took(left)),
                None => format!("over ~{}", took(est)),
            };
            row.push(
                ui::progress()
                    .value((elapsed.as_secs_f64() / est.as_secs_f64()).min(0.99))
                    .label(label)
                    .tone("accent")
                    .min(Bound::w(Extent::Ch(24.0)))
                    .key("e")
                    .into(),
            );
        }
        let mut timer = ui::elapsed().age(ms(state.started_ago(id)));
        if state.outcome.is_some() {
            timer =
                timer.stopped(u64::try_from(state.started_ago(id).as_millis()).unwrap_or(u64::MAX));
        }
        row.push(timer.key("t").into());
        let shown = open == Some(b.drv.as_str());
        let mut node = ui::row()
            .gap("sm")
            .align("center")
            .key(id.to_string())
            .title(if shown {
                "Hide the log"
            } else {
                "Show the log"
            })
            .on_click(Msg::ToggleLog(b.drv.clone()))
            .children(row);
        if shown {
            node = node.mark("pick");
        }
        rows.push(node.into());
        if shown {
            let lines = state.logs.get(&b.drv);
            let skip = lines.map_or(0, |l| l.len().saturating_sub(OPEN_TAIL));
            let text: String = lines
                .into_iter()
                .flatten()
                .skip(skip)
                .map(|l| format!("{l}\x1b[0m\r\n"))
                .collect();
            rows.push(
                ui::ansi(text)
                    .follow(true)
                    .preview(PREVIEW)
                    .max(Bound::h(Extent::Lines(30.0)))
                    .key(format!("{id}.log"))
                    .into(),
            );
        }
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

/// Builds as one bar, beside the download bar for fetched paths: nix fetches
/// and builds at the same time, so the two never share a bar.
fn progress(state: &State) -> Option<Node> {
    if state.outcome.is_some() {
        return None;
    }
    let b = state.build_counts();
    if b.expected == 0 {
        return None;
    }
    let mut label = format!("{}/{} built", b.done, b.expected);
    if b.running > 0 {
        label.push_str(&format!(" · {} running", b.running));
    }
    Some(
        ui::row()
            .gap("sm")
            .align("center")
            .key("bar")
            .children(nodes![
                ui::icon("wrench").tone("accent").key("i"),
                ui::progress()
                    .value((b.done + b.failed) as f64 / b.expected as f64)
                    .label(label)
                    .tone("accent")
                    .grow(1.0)
                    .key("b"),
            ])
            .into(),
    )
}

/// All copies as one bar, in unpacked bytes like nix's own `copied (x/y MiB)`.
fn downloads(state: &State) -> Option<Node> {
    if state.outcome.is_some() {
        return None;
    }
    let (done, total) = state.copy_bytes();
    if total == 0 {
        return None;
    }
    let paths = state.copy_counts();
    let label = format!(
        "{} / {} · {}/{} fetched",
        size(done),
        size(total),
        paths.done,
        paths.expected
    );
    Some(
        ui::row()
            .gap("sm")
            .align("center")
            .key("dl")
            .children(nodes![
                ui::icon("download").tone("info").key("i"),
                ui::progress()
                    .value(done as f64 / total as f64)
                    .label(label)
                    .tone("info")
                    .grow(1.0)
                    .key("b"),
            ])
            .into(),
    )
}

/// `text` with store paths cut down to the name and the path inside it:
/// `/nix/store/<hash>-source/lib/x.nix` reads `source/lib/x.nix`.
fn shorten(text: &str) -> String {
    let is_path = |c: u8| c.is_ascii_alphanumeric() || b"/._+-".contains(&c);
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut copied, mut i) = (0, 0);
    while i + 34 <= b.len() {
        let hash = &b[i + 1..i + 33];
        if b[i] == b'/'
            && b[i + 33] == b'-'
            && hash
                .iter()
                .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
        {
            // Back over the store directory to where the path starts.
            let mut start = i;
            while start > copied && is_path(b[start - 1]) {
                start -= 1;
            }
            out.push_str(&text[copied..start]);
            copied = i + 34;
            i = copied;
        } else {
            i += 1;
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// The `.drv` and exit code of a builder failure, in Lix's and nix's wording.
fn builder_failure(plain: &str) -> Option<(&str, &str)> {
    let (drv, rest) = if let Some((_, rest)) = plain.split_once("builder for '") {
        rest.split_once('\'')?
    } else {
        plain.split_once("Cannot build '")?.1.split_once('\'')?
    };
    let code = rest.split_once("exit code ")?.1;
    let end = code
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(code.len());
    Some((drv, &code[..end]))
}

/// Errors: a builder failure as its name, exit code and its own log tail;
/// anything else as nix printed it, with store paths shortened.
fn errors(state: &State) -> Vec<Node> {
    let mut out = Vec::new();
    for (i, note) in state.notes.iter().enumerate() {
        if note.severity != Severity::Error {
            continue;
        }
        let plain = strip_ansi(&note.text);
        let Some((drv, code)) = builder_failure(&plain) else {
            let text = shorten(&note.text).replace("\n       ", "\n  ");
            out.push(
                ui::ansi(text.replace('\n', "\r\n"))
                    .tone("error")
                    .key(format!("e{i}"))
                    .into(),
            );
            continue;
        };
        // nix quotes the last lines with `> `; the log kept here has more.
        let tail: Vec<&str> = match state.logs.get(drv) {
            Some(log) => log
                .iter()
                .skip(log.len().saturating_sub(LOG_TAIL))
                .map(String::as_str)
                .collect(),
            None => plain
                .lines()
                .filter_map(|l| l.trim_start().strip_prefix("> "))
                .collect(),
        };
        let mut body = nodes![
            ui::row()
                .gap("sm")
                .align("center")
                .key("h")
                .children(nodes![
                    ui::icon("x").tone("error").key("i"),
                    ui::text(vec![
                        ui::span(store_name(drv)).style("error strong"),
                        ui::span(format!(" failed with exit code {code}")).style("error"),
                    ])
                    .key("t"),
                ]),
        ];
        if !tail.is_empty() {
            let text: String = tail.iter().map(|l| format!("{l}\x1b[0m\r\n")).collect();
            body.push(
                ui::ansi(text)
                    .follow(true)
                    .max(Bound::h(Extent::Lines(20.0)))
                    .key("log")
                    .into(),
            );
        }
        body.push(
            ui::text(vec![
                ui::span("nix log ").style("muted"),
                ui::span(drv).style("code"),
            ])
            .key("hint")
            .into(),
        );
        out.push(
            ui::col()
                .gap("xs")
                .tone("error")
                .role("nom.failure")
                .key(format!("e{i}"))
                .children(body)
                .into(),
        );
    }
    out
}

/// `text` with quoted code as code spans: Markdown's `x` and the GNU-style
/// `x' that NixOS option warnings use.
fn code_spans(text: &str) -> Vec<ui::Span> {
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let Some(close) = after.find(['`', '\'']) else {
            break;
        };
        if open > 0 {
            spans.push(ui::span(&rest[..open]));
        }
        spans.push(ui::span(&after[..close]).style("code"));
        rest = &after[close + 1..];
    }
    if !rest.is_empty() {
        spans.push(ui::span(rest));
    }
    spans
}

/// Warnings, deduplicated, one line each, in a folded section.
fn warnings(state: &State) -> Option<Node> {
    let rows: Vec<Node> = state
        .notes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.severity == Severity::Warning)
        .map(|(i, n)| {
            let plain = shorten(&strip_ansi(&n.text));
            let message = plain
                .trim_start()
                .trim_start_matches("evaluation warning:")
                .trim_start_matches("warning:")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let mut spans = code_spans(&message);
            if n.count > 1 {
                spans.push(ui::span(format!("  ×{}", n.count)).style("muted"));
            }
            ui::row()
                .gap("sm")
                .key(format!("w{i}"))
                .children(nodes![
                    ui::icon("warn").tone("warning").key("i"),
                    ui::text(spans).grow(1.0).key("t")
                ])
                .into()
        })
        .collect();
    if rows.is_empty() {
        return None;
    }
    Some(
        ui::section()
            .head(ui::span("Warnings").style("warning"))
            .collapsible(true)
            .collapsed(true)
            .key("warnings")
            .children(rows)
            .into(),
    )
}

/// Order among siblings: what needs attention first.
fn rank(status: DrvStatus) -> u8 {
    match status {
        DrvStatus::Failed => 0,
        DrvStatus::Building(_) => 1,
        DrvStatus::Waiting => 2,
        DrvStatus::Built(_) => 3,
    }
}

/// One derivation and its planned inputs; whether anything in it builds or
/// failed, which keeps it open.
fn dep_item(state: &State, node: &DepNode<'_>) -> (TreeItem, bool) {
    let status = state.status(node.drv);
    let (icon, style, detail) = match status {
        DrvStatus::Waiting if state.outcome.is_some() => ("minus", "muted", "not built".to_owned()),
        DrvStatus::Waiting => (
            "clock",
            "muted",
            state
                .estimate(node.drv, None)
                .map(|e| format!("~{}", took(e)))
                .unwrap_or_default(),
        ),
        DrvStatus::Building(id) => {
            let build = state.builds.get(&id);
            let phase = build.and_then(|b| b.phase.as_deref()).unwrap_or("building");
            let mut detail = format!("{phase} · {}", took(state.started_ago(id)));
            if let Some(est) = build.and_then(|b| b.estimate) {
                detail.push_str(&format!(" / ~{}", took(est)));
            }
            if let Some(machine) = build.and_then(|b| b.machine.as_deref()) {
                detail.push_str(&format!(" · {machine}"));
            }
            ("play", "accent", detail)
        }
        DrvStatus::Built(d) => ("check", "success", took(d)),
        DrvStatus::Failed => ("x", "error", "failed".to_owned()),
    };
    let mut kids: Vec<_> = node.children.iter().collect();
    kids.sort_by_key(|k| rank(state.status(k.drv)));
    let mut active = matches!(status, DrvStatus::Building(_) | DrvStatus::Failed);
    let mut children = Vec::with_capacity(kids.len());
    for kid in kids {
        let (item, kid_active) = dep_item(state, kid);
        active |= kid_active;
        children.push(item);
    }
    let label = vec![
        ui::span(store_name(node.drv)).style(format!("{style} strong")),
        ui::span(format!("  {detail}")).style("muted"),
    ];
    let mut item = TreeItem::new(node.drv, label);
    item.icon = Some(icon.into());
    if !children.is_empty() {
        item.children = children;
        item = item.open(active);
    }
    (item, active)
}

/// What nix builds, as a tree from what was asked for down to its inputs.
fn deps(state: &State) -> Option<Node> {
    let roots = state.dependency_tree();
    if roots.is_empty() {
        return None;
    }
    let only_root = roots.len() == 1;
    let mut tree = ui::tree().key("t");
    for root in &roots {
        let (item, active) = dep_item(state, root);
        let open = active || only_root;
        tree = tree.item(if item.children.is_empty() {
            item
        } else {
            item.open(open)
        });
    }
    let built = state.built.iter().filter(|b| !b.failed).count();
    let head = format!("Derivations · {built}/{} built", state.planned.len());
    Some(
        ui::section()
            .head(head)
            .collapsible(true)
            .collapsed(matches!(state.outcome, Some((Outcome::Done, _))))
            .key("deps")
            .child(tree)
            .into(),
    )
}

/// Paths fetched from caches, folded.
fn fetched(state: &State) -> Option<Node> {
    if state.copied.is_empty() {
        return None;
    }
    let rows: Vec<Node> = state
        .copied
        .iter()
        .enumerate()
        .map(|(i, c)| {
            ui::row()
                .gap("sm")
                .key(format!("c{i}"))
                .children(nodes![
                    ui::icon("download").tone("info").key("i"),
                    ui::text(&c.name).key("n").grow(1.0),
                    ui::text(ui::span(size(c.bytes)).style("muted")).key("s"),
                ])
                .into()
        })
        .collect();
    let bytes: u64 = state.copied.iter().map(|c| c.bytes).sum();
    Some(
        ui::section()
            .head(format!(
                "Fetched · {} paths, {}",
                state.copied.len(),
                size(bytes)
            ))
            .collapsible(true)
            .collapsed(true)
            .key("fetched")
            .children(rows)
            .into(),
    )
}

/// Output nix printed outside any build, folded.
fn output(state: &State) -> Option<Node> {
    let lines = state.logs.get("").filter(|l| !l.is_empty())?;
    let skip = lines.len().saturating_sub(LOG_TAIL);
    let text: String = lines
        .iter()
        .skip(skip)
        .map(|l| format!("{l}\x1b[0m\r\n"))
        .collect();
    Some(
        ui::section()
            .head("Output")
            .collapsible(true)
            .collapsed(true)
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

/// The whole card; `open` is the `.drv` whose log the user opened.
pub fn view(state: &State, title: &str, open: Option<&str>) -> View {
    let status = match state.outcome {
        None => Status::Running,
        Some((Outcome::Done, _)) => Status::Done,
        Some((Outcome::Failed, _)) => Status::Error,
        Some((Outcome::Cancelled, _)) => Status::Cancelled,
    };
    let mut body: Vec<Node> = vec![summary(state)];
    body.extend(progress(state));
    body.extend(downloads(state));
    body.extend(running_builds(state, open));
    body.extend(running_copies(state));
    body.extend(errors(state));
    body.extend(deps(state));
    body.extend(warnings(state));
    body.extend(fetched(state));
    body.extend(output(state));
    View::new().main([ui::card()
        .head(ui::span(title).style("mono"))
        .status(status)
        .role("nom.card")
        .key("nom")
        .children(body)])
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "8nk26gfn51s07v339n5lycablfkl5lj7";

    #[test]
    fn shortens_store_paths_in_quotes_and_after_unicode() {
        let warning = format!(
            "warning: defined in `/nix/store/{HASH}-nix/systems/vm/default.nix' → `/x/{HASH}-a.drv`"
        );
        assert_eq!(
            shorten(&warning),
            "warning: defined in `nix/systems/vm/default.nix' → `a.drv`"
        );
        assert_eq!(shorten("no paths here"), "no paths here");
    }

    #[test]
    fn quotes_code_in_markdown_and_gnu_style() {
        let spans = code_spans("The option `a.b' in `x.nix' is now `c.d`.");
        let code: Vec<_> = spans
            .iter()
            .filter(|s| s.s.as_deref() == Some("code"))
            .map(|s| s.t.as_str())
            .collect();
        assert_eq!(code, ["a.b", "x.nix", "c.d"]);
        assert_eq!(spans.last().map(|s| s.t.as_str()), Some("."));
        // An unclosed quote stays text.
        assert_eq!(code_spans("it`s").len(), 1);
    }

    #[test]
    fn finds_builder_failures_in_lix_and_nix_wording() {
        let lix = format!(
            "error: builder for '/nix/store/{HASH}-c.drv' failed with exit code 3;\n  last 5 log lines:"
        );
        assert_eq!(
            builder_failure(&lix),
            Some((format!("/nix/store/{HASH}-c.drv").as_str(), "3"))
        );
        let nix = format!(
            "error: Cannot build '/nix/store/{HASH}-c.drv'.\n       Reason: builder failed with exit code 12."
        );
        assert_eq!(
            builder_failure(&nix),
            Some((format!("/nix/store/{HASH}-c.drv").as_str(), "12"))
        );
        assert_eq!(
            builder_failure("error: 1 dependencies of derivation failed to build"),
            None
        );
    }
}

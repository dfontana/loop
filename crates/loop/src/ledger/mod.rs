//! The append-only JSONL run record: getting events on and off disk, capturing
//! artifacts, and rendering the digest.
//!
//! See docs/02-how-it-works.md. The contract, in one line: **state is never stored,
//! only folded** — so there is no mutable state file to desync from the log.
//! The fold itself lives in [`crate::core::fold`]; this module is its I/O half.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::core::{CoreError, Event, EventPayload, IoContext, LedgerSink, Result};

pub mod artifacts;
pub mod digest;

pub use artifacts::ArtifactStore;

/// An open ledger file. Appends are `fsync`ed per event; reads tolerate (and
/// discard) a trailing partial line left by a crash mid-write.
pub struct Ledger {
    path: PathBuf,
    /// Kept open in append mode across calls: every write lands at EOF (even
    /// under concurrent writers) and we avoid an open/close per event.
    file: File,
    /// Run time already on the ledger when this handle opened it. Zero for a
    /// fresh run; on `loop resume` it is the last line's `elapsed_s`.
    elapsed_offset_s: u64,
    /// When this handle opened, for the live half of the accumulator.
    opened_at: std::time::Instant,
}

impl Ledger {
    /// Open or create the ledger at `path`, creating parent directories and
    /// repairing a torn trailing line left by a crash mid-write.
    ///
    /// Also reads the run clock forward: whatever the last whole line says the
    /// run has already burned becomes this handle's offset, so a resumed run
    /// keeps counting rather than starting a fresh time budget.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .io_ctx(format!("creating ledger directory {}", parent.display()))?;
        }
        // The repair already read and parsed every line to find out whether the
        // last one was torn, so the clock is read off *that* parse rather than
        // re-reading the file — opening used to cost two full reads and two
        // full parses before the caller's own `read_all` made it three.
        let content = repair_torn_tail(&path)?;
        let elapsed_offset_s = parse_events(&content, &path)?
            .last()
            .map(|e| e.elapsed_s)
            .unwrap_or_default();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .io_ctx(format!("opening ledger {}", path.display()))?;
        Ok(Self {
            path,
            file,
            elapsed_offset_s,
            opened_at: std::time::Instant::now(),
        })
    }

    /// Only this module's own tests read the path back — every caller opened
    /// the ledger by a path it had already computed. Kept because a handle that
    /// can't say what file it is is a bad handle, not because anything needs it.
    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run seconds accumulated before this handle opened. The engine starts
    /// its own budget clock here so the two agree on what "elapsed" means.
    pub fn elapsed_offset_s(&self) -> u64 {
        self.elapsed_offset_s
    }

    /// Whether any events have been written — i.e. a run has started here.
    ///
    /// A byte-length check rather than a full parse: this is a cheap existence
    /// probe (`loop status` on a fresh ticket shouldn't pay for a read+parse),
    /// and an empty file is unambiguously "nothing written yet" regardless of
    /// what a full `read_all` would make of any partial content.
    pub fn started(&self) -> bool {
        fs::metadata(&self.path)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
    }

    /// Read the repaired ledger without reformatting it.
    ///
    /// The parse validates the same byte snapshot that is returned, so a torn
    /// tail appended after this handle opened cannot leak into raw JSONL — the
    /// repair returns the truncated content, and `parse_events` then either
    /// accepts all of it or reports interior corruption.
    pub fn read_raw(&self) -> Result<Vec<u8>> {
        let content = repair_torn_tail(&self.path)?;
        parse_events(&content, &self.path)?;
        Ok(content.into_bytes())
    }
}

impl LedgerSink for Ledger {
    /// Stamp, serialize, append, fsync. One JSON object per line, never
    /// rewritten.
    ///
    /// The line and its trailing newline go out in a single `write_all`, then
    /// `sync_data` blocks until it's durable. If the process dies before the
    /// write completes, the file simply ends without that line (or with an
    /// unparseable fragment of it) — [`Ledger::read_all`] treats that trailing
    /// fragment as a crash artifact, not corruption.
    fn append(&mut self, payload: EventPayload) -> Result<Event> {
        let event = Event::stamped(
            payload,
            self.elapsed_offset_s + self.opened_at.elapsed().as_secs(),
        );
        let mut line = serde_json::to_string(&event)?;
        line.push('\n');
        self.file
            .write_all(line.as_bytes())
            .io_ctx(format!("appending to ledger {}", self.path.display()))?;
        self.file
            .sync_data()
            .io_ctx(format!("fsyncing ledger {}", self.path.display()))?;
        Ok(event)
    }

    /// Every well-formed event in order. A trailing partial line is discarded
    /// (that is a crash mid-write, and costs at most the last event); a
    /// malformed line *in the middle* is an error, because that is corruption.
    fn read_all(&self) -> Result<Vec<Event>> {
        let content = read_content(&self.path)?;
        parse_events(&content, &self.path)
    }
}

fn read_content(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(CoreError::io(
            format!("reading ledger {}", path.display()),
            e,
        )),
    }
}

/// Physically truncate a torn trailing line, so the file on disk is only ever
/// whole events.
///
/// Skipping the torn line at read time is not enough. It is tolerated *because
/// it is last* — so the moment the harness appends the next event, that torn
/// line becomes interior, and every subsequent read fails as corruption. A run
/// interrupted mid-write would resume once and then be permanently unreadable.
///
/// Repairing on open (docs/05-design-notes.md says the reader "tolerates and
/// truncates") makes the crash cost exactly what it should: the one event that
/// was still in flight, and nothing else. Idempotent, so opening a healthy
/// ledger does no I/O beyond the read.
///
/// Returns the repaired contents. It has to read and parse the whole file to
/// decide anything, so handing that back is what lets callers avoid doing the
/// same read a second time.
fn repair_torn_tail(path: &Path) -> Result<String> {
    let content = read_content(path)?;
    if content.is_empty() {
        return Ok(content);
    }

    // Only the final non-empty line may be torn. If an earlier line is also
    // malformed, leave the file untouched so read_all reports the interior
    // corruption instead of silently deleting evidence.
    let mut nonempty = Vec::new();
    let mut offset = 0usize;
    for line in content.split_inclusive('\n') {
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        if !trimmed.trim().is_empty() {
            nonempty.push((offset, serde_json::from_str::<Event>(trimmed).is_ok()));
        }
        offset += line.len();
    }

    let Some((torn_start, torn_valid)) = nonempty.last().copied() else {
        return Ok(content);
    };
    if torn_valid
        || nonempty[..nonempty.len() - 1]
            .iter()
            .any(|(_, valid)| !valid)
    {
        return Ok(content);
    }

    let file = OpenOptions::new()
        .write(true)
        .open(path)
        .io_ctx(format!("opening ledger {} to repair", path.display()))?;
    file.set_len(torn_start as u64)
        .io_ctx(format!("truncating torn tail of {}", path.display()))?;
    file.sync_all()
        .io_ctx(format!("fsyncing repaired ledger {}", path.display()))?;

    let mut content = content;
    content.truncate(torn_start);
    Ok(content)
}

/// Parse newline-delimited events, tolerating an unparseable *last* line (a
/// crash artifact) but erroring on an unparseable line anywhere else (real
/// corruption — see docs/02-how-it-works.md and docs/05-design-notes.md).
fn parse_events(content: &str, path: &Path) -> Result<Vec<Event>> {
    let lines: Vec<&str> = content.lines().collect();
    let last_idx = lines.len().saturating_sub(1);
    let mut events = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Event>(line) {
            Ok(event) => events.push(event),
            Err(e) if i == last_idx => {
                // Trailing unparseable line: the crash-mid-write case. Discard
                // silently — it is at most one lost event, never lost history.
                let _ = e;
                break;
            }
            Err(e) => {
                return Err(CoreError::other(format!(
                    "corrupt ledger line {} of {}: {e}",
                    i + 1,
                    path.display()
                )));
            }
        }
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        Actor, ArtifactRef, Budgets, ErrorKind, GuardOutcome, RunStatus, Totals, Usage,
    };

    fn tmp_ledger() -> (tempfile::TempDir, Ledger) {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path().join("ledger.jsonl")).unwrap();
        (dir, ledger)
    }

    #[test]
    fn open_creates_parent_dirs_and_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("dir").join("ledger.jsonl");
        let ledger = Ledger::open(&path).unwrap();
        assert!(path.exists());
        assert_eq!(ledger.path(), path);
        assert!(!ledger.started());
    }

    #[test]
    fn started_false_until_first_append() {
        let (_dir, mut ledger) = tmp_ledger();
        assert!(!ledger.started());
        ledger
            .append(EventPayload::Note { text: "hi".into() })
            .unwrap();
        assert!(ledger.started());
    }

    /// Round-trip every `EventPayload` variant through append -> read_all.
    #[test]
    fn round_trips_every_event_variant() {
        let (_dir, mut ledger) = tmp_ledger();

        let payloads = vec![
            EventPayload::RunStarted {
                ticket: "PROJ-1".into(),
                machine_hash: "sha256:abc".into(),
                budgets: Budgets {
                    usd: Some(8.0),
                    wallclock_s: Some(5400),
                    max_transitions: Some(40),
                },
            },
            EventPayload::StateEntered {
                state: "implement".into(),
                cycle: 1,
                attempt: 1,
                session_id: Some("sess-1".into()),
                model: "claude-sonnet-5".into(),
                thinking: "high".into(),
                skills: vec!["spark-build".into()],
                mcp: vec![],
            },
            EventPayload::WorkerOutput {
                state: "implement".into(),
                cycle: 1,
                summary: "Added the field.".into(),
                artifacts: vec![ArtifactRef {
                    name: "diff".into(),
                    path: ".loop/artifacts/implement-1-diff.patch".into(),
                }],
                usage: Usage {
                    tokens: 100,
                    cost_usd: 0.5,
                },
            },
            EventPayload::TransitionProposed {
                from: "implement".into(),
                to: Some("review".into()),
                blocked: false,
                rationale: "Done.".into(),
                by: Actor::Worker,
            },
            EventPayload::GuardChecked {
                from: "implement".into(),
                to: "review".into(),
                structural: GuardOutcome::Pass,
                check: GuardOutcome::Pass,
                criteria: GuardOutcome::Pass,
                check_output: Some("build: OK".into()),
                judge_rationale: Some("Looks fine.".into()),
                usage: Usage {
                    tokens: 900,
                    cost_usd: 0.02,
                },
            },
            EventPayload::NavigatorInvoked {
                from: "implement".into(),
                proposal: "blocked".into(),
                chosen_to: "debug".into(),
                entry_prompt: Some("Get back on track.".into()),
                usage: Usage {
                    tokens: 10,
                    cost_usd: 0.01,
                },
            },
            EventPayload::TransitionCommitted {
                from: "implement".into(),
                to: "review".into(),
                cycle: 1,
            },
            EventPayload::Error {
                state: Some("qa".into()),
                kind: ErrorKind::Transient,
                detail: "executor lost".into(),
            },
            EventPayload::Note {
                text: "human annotation".into(),
            },
            EventPayload::RunFinished {
                status: RunStatus::Done,
                terminal_state: Some("done".into()),
                totals: Totals {
                    cost_usd: 5.1,
                    wallclock_s: 3654,
                    transitions: 11,
                },
            },
        ];

        for p in &payloads {
            ledger.append(p.clone()).unwrap();
        }

        let read = ledger.read_all().unwrap();
        assert_eq!(read.len(), payloads.len());
        for (event, payload) in read.iter().zip(payloads.iter()) {
            assert_eq!(event.kind(), payload.kind());
            // Round-trip through JSON once more and compare structurally.
            let got = serde_json::to_value(&event.payload).unwrap();
            let want = serde_json::to_value(payload).unwrap();
            assert_eq!(got, want);
        }
    }

    #[test]
    fn append_is_one_json_object_per_line_and_never_rewrites() {
        let (_dir, mut ledger) = tmp_ledger();
        ledger
            .append(EventPayload::Note { text: "one".into() })
            .unwrap();
        ledger
            .append(EventPayload::Note { text: "two".into() })
            .unwrap();
        let content = fs::read_to_string(ledger.path()).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        for line in &lines {
            serde_json::from_str::<Event>(line).expect("each line is standalone JSON");
        }
        assert!(content.ends_with('\n'));
    }

    #[test]
    fn fixture_clean_run_reads_back_in_order() {
        let (_dir, mut ledger) = tmp_ledger();
        ledger
            .append(EventPayload::RunStarted {
                ticket: "T-1".into(),
                machine_hash: "sha256:1".into(),
                budgets: Budgets::default(),
            })
            .unwrap();
        ledger
            .append(EventPayload::StateEntered {
                state: "implement".into(),
                cycle: 1,
                attempt: 1,
                session_id: None,
                model: "m".into(),
                thinking: "high".into(),
                skills: vec![],
                mcp: vec![],
            })
            .unwrap();
        ledger
            .append(EventPayload::WorkerOutput {
                state: "implement".into(),
                cycle: 1,
                summary: "done".into(),
                artifacts: vec![],
                usage: Usage::default(),
            })
            .unwrap();
        ledger
            .append(EventPayload::TransitionCommitted {
                from: "implement".into(),
                to: "done".into(),
                cycle: 1,
            })
            .unwrap();
        ledger
            .append(EventPayload::RunFinished {
                status: RunStatus::Done,
                terminal_state: Some("done".into()),
                totals: Totals::default(),
            })
            .unwrap();

        let events = ledger.read_all().unwrap();
        let kinds: Vec<_> = events.iter().map(|e| e.kind()).collect();
        assert_eq!(
            kinds,
            vec![
                "run_started",
                "state_entered",
                "worker_output",
                "transition_committed",
                "run_finished"
            ]
        );
    }

    #[test]
    fn fixture_crash_mid_stage_has_state_entered_with_no_worker_output() {
        let (_dir, mut ledger) = tmp_ledger();
        ledger
            .append(EventPayload::RunStarted {
                ticket: "T-1".into(),
                machine_hash: "sha256:1".into(),
                budgets: Budgets::default(),
            })
            .unwrap();
        ledger
            .append(EventPayload::StateEntered {
                state: "implement".into(),
                cycle: 1,
                attempt: 1,
                session_id: Some("sess-1".into()),
                model: "m".into(),
                thinking: "high".into(),
                skills: vec![],
                mcp: vec![],
            })
            .unwrap();
        // Crash here: no worker_output follows.

        let events = ledger.read_all().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events.last().unwrap().kind(), "state_entered");
    }

    #[test]
    fn fixture_run_finished_is_terminal_and_read_back() {
        let (_dir, mut ledger) = tmp_ledger();
        ledger
            .append(EventPayload::RunFinished {
                status: RunStatus::Failed,
                terminal_state: None,
                totals: Totals::default(),
            })
            .unwrap();
        let events = ledger.read_all().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0].payload {
            EventPayload::RunFinished { status, .. } => assert_eq!(*status, RunStatus::Failed),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn truncated_final_line_is_discarded_without_error() {
        let (_dir, mut ledger) = tmp_ledger();
        ledger
            .append(EventPayload::Note { text: "one".into() })
            .unwrap();
        ledger
            .append(EventPayload::Note { text: "two".into() })
            .unwrap();

        // Simulate a crash mid-write: append a truncated, unparseable JSON
        // fragment with no trailing newline.
        let mut file = OpenOptions::new().append(true).open(ledger.path()).unwrap();
        write!(
            file,
            "{{\"ts\":\"2026-01-01T00:00:00Z\",\"type\":\"note\",\"tex"
        )
        .unwrap();
        file.sync_data().unwrap();
        drop(file);

        let events = ledger.read_all().unwrap();
        assert_eq!(
            events.len(),
            2,
            "the partial trailing line must be dropped, not erroring"
        );
    }

    #[test]
    fn corrupt_line_in_the_middle_is_an_error() {
        let (_dir, mut ledger) = tmp_ledger();
        ledger
            .append(EventPayload::Note { text: "one".into() })
            .unwrap();

        {
            let mut file = OpenOptions::new().append(true).open(ledger.path()).unwrap();
            writeln!(file, "not even close to json").unwrap();
            file.sync_data().unwrap();
        }

        ledger
            .append(EventPayload::Note {
                text: "three".into(),
            })
            .unwrap();

        let err = ledger.read_all().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("corrupt"), "unexpected message: {msg}");
    }

    /// The run clock has to survive the process that was keeping it. A second
    /// handle over the same file — which is exactly what `loop resume` opens —
    /// must pick the accumulator up from the last line rather than restart it,
    /// or every resumed run gets a fresh wallclock budget.
    #[test]
    fn a_reopened_ledger_resumes_the_run_clock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.jsonl");
        {
            let mut ledger = Ledger::open(&path).unwrap();
            assert_eq!(ledger.elapsed_offset_s(), 0);
            // Forge a line claiming an hour of run time, the way a long first
            // session would have left it.
            let mut line = serde_json::to_string(&Event {
                ts: "2026-07-24T00:00:00.000Z".into(),
                elapsed_s: 3600,
                payload: EventPayload::Note {
                    text: "an hour in".into(),
                },
            })
            .unwrap();
            line.push('\n');
            ledger.file.write_all(line.as_bytes()).unwrap();
            ledger.file.sync_data().unwrap();
        }

        let mut resumed = Ledger::open(&path).unwrap();
        assert_eq!(resumed.elapsed_offset_s(), 3600);
        let event = resumed
            .append(EventPayload::Note {
                text: "after resume".into(),
            })
            .unwrap();
        assert!(
            event.elapsed_s >= 3600,
            "the resumed run must keep counting from 3600, got {}",
            event.elapsed_s
        );
    }

    #[test]
    fn read_all_on_missing_file_is_empty_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.jsonl");
        // Construct without calling `open` (which would create it) by writing
        // directly against a Ledger built over a path we then delete.
        let ledger = Ledger::open(&missing).unwrap();
        fs::remove_file(&missing).unwrap();
        assert!(ledger.read_all().unwrap().is_empty());
    }
}

#[cfg(test)]
mod repair_tests {
    use super::*;

    fn torn_ledger() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.jsonl");
        {
            let mut l = Ledger::open(&path).unwrap();
            l.append(EventPayload::Note { text: "one".into() }).unwrap();
            l.append(EventPayload::Note { text: "two".into() }).unwrap();
        }
        // A write that died partway through.
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"{\"ts\":\"2026-07-24T00:00:00Z\",\"type\":\"worker_ou")
            .unwrap();
        f.sync_data().unwrap();
        (dir, path)
    }

    /// The regression that motivated repairing on open: skipping the torn line
    /// only works while it is *last*. Append after it and it becomes interior,
    /// so every later read fails and the run can never be resumed again.
    #[test]
    fn appending_after_a_torn_tail_keeps_the_ledger_readable() {
        let (_dir, path) = torn_ledger();

        let mut ledger = Ledger::open(&path).unwrap();
        assert_eq!(ledger.read_all().unwrap().len(), 2);

        ledger
            .append(EventPayload::Note {
                text: "after the crash".into(),
            })
            .unwrap();

        let events = ledger.read_all().expect("must still read after appending");
        assert_eq!(events.len(), 3);

        // And again, through a fresh handle, the way `loop resume` would.
        let reopened = Ledger::open(&path).unwrap();
        assert_eq!(reopened.read_all().unwrap().len(), 3);
    }

    #[test]
    fn repair_truncates_only_the_torn_line() {
        let (_dir, path) = torn_ledger();
        let before = fs::read_to_string(&path).unwrap();
        assert!(before.contains("worker_ou"));

        let _ledger = Ledger::open(&path).unwrap();

        let after = fs::read_to_string(&path).unwrap();
        assert!(
            !after.contains("worker_ou"),
            "torn line survived: {after:?}"
        );
        assert_eq!(after.lines().count(), 2);
        assert!(after.ends_with('\n'));
    }

    #[test]
    fn interior_corruption_is_not_removed_with_a_torn_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.jsonl");
        {
            let mut ledger = Ledger::open(&path).unwrap();
            ledger
                .append(EventPayload::Note { text: "one".into() })
                .unwrap();
        }
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"not-json\n{\"type\":\"note\"").unwrap();
        file.sync_data().unwrap();
        drop(file);
        let before = fs::read(&path).unwrap();

        assert!(Ledger::open(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn raw_read_drops_a_torn_tail_added_after_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.jsonl");
        let mut ledger = Ledger::open(&path).unwrap();
        ledger
            .append(EventPayload::Note { text: "one".into() })
            .unwrap();
        let expected = fs::read(&path).unwrap();

        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"{\"type\":\"note\"").unwrap();
        file.sync_data().unwrap();
        drop(file);

        assert_eq!(ledger.read_raw().unwrap(), expected);
        assert_eq!(fs::read(&path).unwrap(), expected);
    }

    #[test]
    fn repair_is_a_no_op_on_a_healthy_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.jsonl");
        {
            let mut l = Ledger::open(&path).unwrap();
            l.append(EventPayload::Note { text: "one".into() }).unwrap();
        }
        let before = fs::read_to_string(&path).unwrap();
        let _ledger = Ledger::open(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }
}

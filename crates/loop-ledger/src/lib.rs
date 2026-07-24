//! The append-only JSONL run record: getting events on and off disk, capturing
//! artifacts, and rendering the digest.
//!
//! See docs/03-ledger.md. The contract, in one line: **state is never stored,
//! only folded** — so there is no mutable state file to desync from the log.
//! The fold itself lives in `loop_core::fold`; this crate is its I/O half.
//!
//! TASK T1 implements this crate. The signatures below are the contract the
//! engine is already written against; fill them in, don't reshape them.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use loop_core::{CoreError, Event, EventPayload, IoContext, LedgerSink, Result, RunState};

pub mod artifacts;
pub mod digest;

pub use artifacts::ArtifactStore;
pub use loop_core::{FoldStatus, ResumePoint, RunState as FoldedState};

/// An open ledger file. Appends are `fsync`ed per event; reads tolerate (and
/// discard) a trailing partial line left by a crash mid-write.
pub struct Ledger {
    path: PathBuf,
    /// Kept open in append mode across calls: every write lands at EOF (even
    /// under concurrent writers) and we avoid an open/close per event.
    file: File,
}

impl Ledger {
    /// Open or create the ledger at `path`, creating parent directories.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .io_ctx(format!("creating ledger directory {}", parent.display()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .io_ctx(format!("opening ledger {}", path.display()))?;
        Ok(Self { path, file })
    }

    pub fn path(&self) -> &Path {
        &self.path
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

    /// Read and fold in one step.
    pub fn fold(&self) -> Result<RunState> {
        Ok(loop_core::fold(&self.read_all()?))
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
        let event = Event::now(payload);
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
        let content = match fs::read_to_string(&self.path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(CoreError::io(
                    format!("reading ledger {}", self.path.display()),
                    e,
                ));
            }
        };
        parse_events(&content, &self.path)
    }
}

/// Parse newline-delimited events, tolerating an unparseable *last* line (a
/// crash artifact) but erroring on an unparseable line anywhere else (real
/// corruption — see docs/03-ledger.md and docs/07-risks.md #9).
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
    use loop_core::{
        Actor, ArtifactRef, Budgets, ErrorKind, GuardOutcome, RunStatus, Totals, Usage,
    };
    use serde_json::json;

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
                resolved_config: json!({"playbooks": {"implement": "sha256:1"}}),
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
                tools: vec!["read".into(), "edit".into(), "transition".into()],
            },
            EventPayload::WorkerOutput {
                state: "implement".into(),
                cycle: 1,
                summary: "Added the field.".into(),
                artifacts: vec![ArtifactRef {
                    name: "diff".into(),
                    path: ".loop/artifacts/implement-1-diff.patch".into(),
                    sha256: "deadbeef".into(),
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
                when: GuardOutcome::Skip,
                criteria: GuardOutcome::Pass,
                judge_rationale: Some("Looks fine.".into()),
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
            EventPayload::VarsSet {
                scope: Some("build".into()),
                values: json!({"build": {"status": "pass"}}),
                trusted: true,
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
                resolved_config: json!({}),
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
                tools: vec![],
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
                resolved_config: json!({}),
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
                tools: vec![],
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

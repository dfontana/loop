//! Running a transition's deterministic check.
//!
//! The harness shells out for itself here: no agent, no session, no transcript
//! anywhere in the path. That is the whole point of the tier — it is the one
//! signal a worker cannot author, so it must not be routed through anything a
//! worker touches.
//!
//! Output is captured to a temp file rather than a pipe. A check is free to
//! print a whole build log, and a pipe nobody drains while we wait on the
//! child deadlocks once the buffer fills — the exact failure mode a timeout is
//! supposed to rescue us from.

use std::io::Read as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use loop_core::{CheckOutcome, CoreError, IoContext, Result};

/// Cap on captured output, in bytes. A check that prints a 200 MB log should
/// not balloon the ledger or the Judge's prompt; the tail is what a human (or
/// a Judge) reads anyway, so keep the tail.
const MAX_OUTPUT_BYTES: usize = 16 * 1024;

/// How often to poll for exit while waiting out the timeout.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Run `cmd` via `bash -c` in `cwd`, with `env` exported, bounded by
/// `timeout_s`.
///
/// A non-zero exit is a *check failure*, not a harness error: it is the answer
/// the tier asked for, so it comes back as `Ok(CheckOutcome { passed: false })`.
/// Only a failure to run the command at all is an `Err`.
pub fn exec_check(
    cmd: &str,
    cwd: &Path,
    env: &[(String, String)],
    timeout_s: u64,
) -> Result<CheckOutcome> {
    let mut capture = tempfile::tempfile().io_ctx("creating check output buffer")?;
    let err_handle = capture
        .try_clone()
        .io_ctx("duplicating check output buffer")?;
    let out_handle = capture
        .try_clone()
        .io_ctx("duplicating check output buffer")?;

    let mut command = Command::new("bash");
    command
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out_handle))
        .stderr(Stdio::from(err_handle));
    for (k, v) in env {
        command.env(k, v);
    }

    let mut child = command
        .spawn()
        .map_err(|e| CoreError::io(format!("spawning check `{cmd}`"), e))?;

    let deadline = Instant::now() + Duration::from_secs(timeout_s);
    let (exit_code, timed_out) = loop {
        match child
            .try_wait()
            .map_err(|e| CoreError::io(format!("waiting on check `{cmd}`"), e))?
        {
            Some(status) => break (status.code(), false),
            None if Instant::now() >= deadline => {
                // Best-effort kill; whatever it wrote before the deadline is
                // still in the capture file and worth reporting.
                let _ = child.kill();
                let _ = child.wait();
                break (None, true);
            }
            None => std::thread::sleep(POLL_INTERVAL),
        }
    };

    let mut output = String::new();
    use std::io::Seek as _;
    capture.rewind().io_ctx("rewinding check output buffer")?;
    let mut raw = Vec::new();
    capture
        .read_to_end(&mut raw)
        .io_ctx("reading check output")?;
    output.push_str(&tail_lossy(&raw, MAX_OUTPUT_BYTES));

    if timed_out {
        output.push_str(&format!("\n[check timed out after {timeout_s}s]"));
    }

    Ok(CheckOutcome {
        passed: !timed_out && exit_code == Some(0),
        exit_code,
        output: output.trim().to_string(),
    })
}

/// The last `max` bytes, decoded lossily, prefixed with a truncation note when
/// anything was dropped. Splitting on a byte boundary mid-codepoint is what
/// `from_utf8_lossy` is for.
fn tail_lossy(raw: &[u8], max: usize) -> String {
    if raw.len() <= max {
        return String::from_utf8_lossy(raw).into_owned();
    }
    let dropped = raw.len() - max;
    let tail = String::from_utf8_lossy(&raw[dropped..]);
    format!("[… {dropped} earlier bytes truncated …]\n{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(cmd: &str) -> CheckOutcome {
        exec_check(cmd, Path::new("."), &[], 10).expect("check ran")
    }

    #[test]
    fn zero_exit_passes_and_captures_stdout() {
        let outcome = run("echo hello");
        assert!(outcome.passed);
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(outcome.output, "hello");
    }

    /// A failing check is an answer, not an error — the tier asked a question
    /// and got "no". Surfacing it as `Err` would abort the run instead of
    /// letting the edge's `on_fail` decide.
    #[test]
    fn nonzero_exit_fails_without_erroring() {
        let outcome = run("echo 'boom' >&2; exit 3");
        assert!(!outcome.passed);
        assert_eq!(outcome.exit_code, Some(3));
        assert_eq!(outcome.output, "boom");
    }

    #[test]
    fn stdout_and_stderr_are_both_captured() {
        let outcome = run("echo out; echo err >&2");
        assert!(outcome.output.contains("out"));
        assert!(outcome.output.contains("err"));
    }

    #[test]
    fn env_is_exported_to_the_check() {
        let outcome = exec_check(
            "echo \"$TICKET_ID/$CYCLE\"",
            Path::new("."),
            &[
                ("TICKET_ID".to_string(), "PROJ-7".to_string()),
                ("CYCLE".to_string(), "2".to_string()),
            ],
            10,
        )
        .unwrap();
        assert_eq!(outcome.output, "PROJ-7/2");
    }

    #[test]
    fn runs_in_the_given_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker.txt"), "x").unwrap();
        let outcome = exec_check("ls marker.txt", dir.path(), &[], 10).unwrap();
        assert!(outcome.passed, "got {outcome:?}");
    }

    /// A check that hangs must not hang the run. It is killed, reported as a
    /// failure, and says so in the output rather than looking like a clean
    /// non-zero exit.
    #[test]
    fn a_hanging_check_times_out_and_fails() {
        let outcome = exec_check("sleep 30", Path::new("."), &[], 1).unwrap();
        assert!(!outcome.passed);
        assert_eq!(outcome.exit_code, None);
        assert!(outcome.output.contains("timed out"), "got {outcome:?}");
    }

    /// A check free to print a build log must not be able to balloon the
    /// ledger or the Judge's prompt.
    #[test]
    fn oversized_output_is_truncated_from_the_front() {
        let outcome = exec_check(
            "for i in $(seq 1 5000); do echo 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'; done; echo TAIL",
            Path::new("."),
            &[],
            30,
        )
        .unwrap();
        assert!(outcome.passed);
        assert!(outcome.output.len() < MAX_OUTPUT_BYTES + 200);
        assert!(outcome.output.contains("truncated"));
        assert!(
            outcome.output.trim_end().ends_with("TAIL"),
            "the tail is the part worth keeping"
        );
    }
}

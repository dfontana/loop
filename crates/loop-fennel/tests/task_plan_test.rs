//! `:task`/`:plan` resolution: an existing relative path is read as a file, a
//! non-path string is kept as inline prose, and a `.md`-suffixed value that
//! doesn't resolve is a clear error naming the path searched.

mod common;

use std::fs;

use loop_core::CoreError;

fn machine_with_task(dir: &std::path::Path, task_value: &str) -> std::path::PathBuf {
    let content = format!(
        r#"{{:ticket "T"
 :task "{task_value}"
 :plan "inline plan text, nothing fancy"
 :entry "a"
 :terminals ["done"]
 :states {{:a {{:playbook "a"}}}}
 :transitions []}}
"#
    );
    let path = dir.join("machine.fnl");
    fs::write(&path, content).unwrap();
    path
}

#[test]
fn task_as_existing_file_path_is_read() {
    let vm = common::vm();
    let config = common::default_config();
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("task.md"), "Do the actual work.").unwrap();
    let path = machine_with_task(tmp.path(), "task.md");

    let machine = vm.load_machine(&path, &config).expect("load_machine");
    assert_eq!(machine.task, "Do the actual work.");
}

#[test]
fn task_as_non_path_string_is_inline_prose() {
    let vm = common::vm();
    let config = common::default_config();
    let tmp = tempfile::tempdir().unwrap();
    let path = machine_with_task(tmp.path(), "Just fix the flaky test, no ticket needed");

    let machine = vm.load_machine(&path, &config).expect("load_machine");
    assert_eq!(machine.task, "Just fix the flaky test, no ticket needed");
}

#[test]
fn task_with_md_suffix_that_does_not_resolve_is_an_error() {
    let vm = common::vm();
    let config = common::default_config();
    let tmp = tempfile::tempdir().unwrap();
    let path = machine_with_task(tmp.path(), "missing.md");

    let err = vm.load_machine(&path, &config).unwrap_err();
    match &err {
        CoreError::Unresolved { name, searched, .. } => {
            assert_eq!(name, "missing.md");
            assert_eq!(searched.len(), 1);
            assert_eq!(searched[0], tmp.path().join("missing.md"));
        }
        other => panic!("expected CoreError::Unresolved, got {other:?}"),
    }
    // The Display impl also names the searched path — this is what a human
    // actually sees on the terminal.
    let msg = err.to_string();
    assert!(msg.contains("missing.md"));
}

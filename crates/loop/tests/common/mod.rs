#![allow(dead_code)] // shared helpers: not every test binary uses every one.

use std::path::{Path, PathBuf};

use r#loop::core::Paths;
use r#loop::fennel::FennelVm;

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The `mock-pi` fixture binary.
///
/// It is a sibling workspace member rather than a dependency, so cargo exports
/// no `CARGO_BIN_EXE_mock-pi` — but it is built into the same directory as the
/// binary under test, which cargo *does* export. Locating it that way costs
/// nothing and is correct under every profile.
///
/// `mock_pi_e2e.rs` used to shell out to `cargo build -p mock-pi` and then
/// guess `target/debug/`, which rebuilt on every run and looked in the wrong
/// place under `--release`.
pub fn mock_pi() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_BIN_EXE_loop")).with_file_name("mock-pi");
    assert!(
        path.exists(),
        "mock-pi not built at {}; run `cargo build --workspace`",
        path.display()
    );
    path
}

pub fn vm() -> FennelVm {
    FennelVm::new().expect("FennelVm::new")
}

/// `Paths` pointing nowhere real — fine for tests that never touch the tree.
pub fn test_paths() -> Paths {
    Paths {
        project_dir: PathBuf::from("/nonexistent/project"),
    }
}

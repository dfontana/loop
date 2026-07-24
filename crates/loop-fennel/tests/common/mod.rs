#![allow(dead_code)] // shared helpers: not every test binary uses every one.

use std::path::{Path, PathBuf};

use loop_core::{Config, Paths};
use loop_fennel::FennelVm;

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

pub fn vm() -> FennelVm {
    FennelVm::new().expect("FennelVm::new")
}

/// `Paths` pointing nowhere real — fine for tests that only care about
/// `Config::defaults`' non-path fields, or that override `config_dir`
/// themselves.
pub fn test_paths() -> Paths {
    Paths {
        config_dir: PathBuf::from("/nonexistent/config"),
        state_dir: PathBuf::from("/nonexistent/state"),
        project_dir: PathBuf::from("/nonexistent/project"),
    }
}

pub fn default_config() -> Config {
    Config::defaults(test_paths())
}

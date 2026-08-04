#![allow(dead_code)] // shared helpers: not every test binary uses every one.

use std::path::{Path, PathBuf};

use r#loop::core::{Config, Paths};
use r#loop::fennel::FennelVm;

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

pub fn vm() -> FennelVm {
    FennelVm::new().expect("FennelVm::new")
}

/// `Paths` pointing nowhere real — fine for tests that only care about
/// `Config::defaults`' non-path fields.
pub fn test_paths() -> Paths {
    Paths {
        project_dir: PathBuf::from("/nonexistent/project"),
    }
}

pub fn default_config() -> Config {
    Config::defaults(test_paths())
}

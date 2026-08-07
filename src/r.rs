use std::path::PathBuf;

use anyhow::{Context, Result};

pub const R_LIB_SITE: &str = "/opt/r-library";

pub const R_CRAN_REPO: &str = "https://cloud.r-project.org";

/// XDG base dirs used by both opencode and R inside the container. Kept in one
/// place so the run and R setup containers stay consistent.
pub const XDG_ENV: [(&str, &str); 4] = [
    ("XDG_CONFIG_HOME", "/opencode/config"),
    ("XDG_CACHE_HOME", "/opencode/cache"),
    ("XDG_DATA_HOME", "/opencode/data"),
    ("XDG_STATE_HOME", "/opencode/state"),
];

pub fn r_user_dir_mounts() -> Result<Vec<(PathBuf, String)>> {
    let cache = dirs::cache_dir().context("cache dir required")?.join("R");
    let data = dirs::data_dir().context("data dir required")?.join("R");
    let config = dirs::config_dir().context("config dir required")?.join("R");
    let state = match dirs::state_dir() {
        Some(s) => s,
        None => dirs::home_dir()
            .context("home dir required")?
            .join(".local/state"),
    };
    let state = state.join("R");
    for dir in [&cache, &data, &config, &state] {
        let _ = std::fs::create_dir_all(dir);
    }
    Ok(vec![
        (cache, "/opencode/cache/R".to_string()),
        (data, "/opencode/data/R".to_string()),
        (config, "/opencode/config/R".to_string()),
        (state, "/opencode/state/R".to_string()),
    ])
}

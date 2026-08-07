use anyhow::{Context, Result};
use std::path::PathBuf;

pub fn resolve_path(s: &str) -> Result<PathBuf> {
    let expanded =
        shellexpand::full(s).with_context(|| format!("failed to expand path '{}'", s))?;
    std::fs::canonicalize(expanded.as_ref())
        .with_context(|| format!("cannot resolve path '{}'", expanded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_home_dir() {
        let home = dirs::home_dir().expect("home dir");
        assert_eq!(resolve_path("~").unwrap(), home);
    }

    #[test]
    fn resolve_env_var_path() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("subdir");
        std::fs::create_dir(&p).unwrap();
        std::env::set_var("_OD_TEST", tmp.path());
        let result = resolve_path("${_OD_TEST}/subdir").unwrap();
        assert_eq!(result, p.canonicalize().unwrap());
    }

    #[test]
    fn resolve_absolute_path() {
        let result = resolve_path("/tmp").unwrap();
        assert!(result.exists());
    }

    #[test]
    fn resolve_nonexistent_fails() {
        assert!(resolve_path("/nonexistent_xyz_opencode_999999").is_err());
    }
}

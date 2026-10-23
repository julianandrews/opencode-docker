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
    use std::ffi::OsString;
    use std::sync::Mutex;

    static HOME_LOCK: Mutex<()> = Mutex::new(());

    struct HomeGuard(Option<OsString>);

    impl HomeGuard {
        fn set(home: &std::path::Path) -> Self {
            let old = std::env::var_os("HOME");
            std::env::set_var("HOME", home);
            Self(old)
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(old) => std::env::set_var("HOME", old),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    #[test]
    fn resolve_home_dir() {
        // The Nix build sandbox sets HOME=/homeless-shelter, which does not
        // exist. Use a real temporary home so the test is hermetic.
        let _lock = HOME_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let _home = HomeGuard::set(tmp.path());

        assert_eq!(
            resolve_path("~").unwrap(),
            tmp.path().canonicalize().unwrap()
        );
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

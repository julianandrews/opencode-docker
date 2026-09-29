use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::path;
use crate::r;
use crate::util;

/// Default `opencode.jsonc` seeded into the host config dir on first run
/// (source: `docker/opencode.jsonc.template`). The file is bind-mounted
/// read-only into the container, so an agent can never rewrite its own
/// permissions, and the `deny` policies inside survive any project config.
const DEFAULT_OPENCODE_CONFIG: &str = include_str!("../docker/opencode.jsonc.template");

#[derive(Debug)]
pub struct DockerArgs {
    args: Vec<String>,
}

impl DockerArgs {
    pub fn new() -> Self {
        DockerArgs { args: Vec::new() }
    }

    pub fn into_vec(self) -> Vec<String> {
        self.args
    }

    pub fn base(&mut self) -> &mut Self {
        let uid = util::uid();
        let gid = util::gid();
        let term = std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".to_string());
        let lang = std::env::var("LANG").unwrap_or_else(|_| "C.UTF-8".to_string());

        self.args.push("--rm".into());
        self.args.push("-it".into());
        self.args.push("--init".into());
        self.args.push("--hostname".into());
        self.args.push("opencode".into());
        self.args.push("--user".into());
        self.args.push(format!("{}:{}", uid, gid));
        self.args.push("-e".into());
        self.args.push(format!("TERM={}", term));
        self.args.push("-e".into());
        self.args.push("COLORTERM=truecolor".into());
        self.args.push("-e".into());
        self.args.push(format!("LANG={}", lang));
        self
    }

    /// `edit_config` is the deliberate `--edit-config` escape hatch: the config
    /// files and the `plugins`/`node_modules` dirs are then left writable (no
    /// read-only mounts) so config and plugins can be edited from inside the
    /// container. `config_dir` is the host XDG config dir; it is injected so
    /// the tests never touch the real one.
    pub fn xdg_dirs(&mut self, config_dir: &Path, edit_config: bool) -> Result<&mut Self> {
        let config = config_dir.to_path_buf();
        let cache = dirs::cache_dir().context("cache dir required")?;
        let data = dirs::data_dir().context("data dir required")?;
        let state = match dirs::state_dir() {
            Some(s) => s,
            None => {
                let home = dirs::home_dir().context("home dir required")?;
                home.join(".local/state")
            }
        };

        let dirs_to_create = [
            // Created host-side so the bind mount is owned by the host user:
            // Docker would otherwise create it as root and the container's
            // non-root user could not write cli.json/service.json (V2 mounts
            // the config dir read-write for its first-run tui.json migration).
            config.join("opencode"),
            // The plugins dir and node_modules are created (and later mounted
            // read-only) so writable targets for plugin code never exist in
            // the container; see the mounts in xdg_dirs.
            config.join("opencode/plugins"),
            config.join("opencode/node_modules"),
            cache.join("opencode"),
            data.join("opencode"),
            data.join("opencode/log"),
            data.join("opencode/bin"),
            data.join("opentui"),
            state.join("opencode"),
            data.join("opencode-docker/r-library"),
        ];
        for dir in &dirs_to_create {
            let _ = std::fs::create_dir_all(dir);
        }

        for (name, value) in r::XDG_ENV {
            self.args.push("-e".into());
            self.args.push(format!("{}={}", name, value));
        }
        self.args.push("-v".into());
        self.args.push(format!(
            "{}:/opencode/config/opencode",
            config.join("opencode").display()
        ));
        // Seed a default opencode config host-side when none exists yet (the
        // host-side creation keeps the bind mount owned by the host user, like
        // the dirs above). By default all existing global config files, the
        // plugins dir, node_modules and the package manifests are then
        // additionally mounted read-only: an agent must never be able to
        // rewrite its own permissions or persist code that auto-loads on the
        // next launch. `--edit-config` drops these mounts so config and
        // plugins can be edited in-container. cli.json/service.json stay
        // writable for the TUI either way.
        let config_dir = config.join("opencode");
        if edit_config {
            let _ = seed_opencode_config(&config_dir);
        } else {
            for mount in opencode_config_mounts(&config_dir)? {
                self.args.push("-v".into());
                self.args.push(mount);
            }
            // opencode auto-discovers plugins dropped into its global plugins
            // dir (any .ts/.js is loaded on the next launch), so the dir is
            // mounted read-only: a writable plugins dir would let an agent
            // persist arbitrary code in the host config dir. Plugin management
            // therefore happens host-side. The mounts are only pushed when the
            // host-side dirs exist: mounting a nonexistent source would make
            // Docker create a root-owned directory in the user's config tree.
            push_ro_dir_mount(self, &config_dir.join("plugins"), "plugins");
            // Same for the npm plugin surface: node_modules and the package
            // manifests are read-only in locked mode, so plugin packages can
            // only be installed host-side (or in a --edit-config session).
            // opencode never needs to write them at runtime; a plugin declared
            // in config but not installed host-side just fails to auto-load.
            push_ro_dir_mount(self, &config_dir.join("node_modules"), "node_modules");
            for mount in existing_file_mounts(
                &config_dir,
                &["package.json", "package-lock.json", "bun.lock"],
            ) {
                self.args.push("-v".into());
                self.args.push(mount);
            }
        }
        self.args.push("-v".into());
        self.args.push(format!(
            "{}:/opencode/cache/opencode",
            cache.join("opencode").display()
        ));
        self.args.push("-v".into());
        self.args.push(format!(
            "{}:/opencode/data/opencode",
            data.join("opencode").display()
        ));
        // opentui (the TUI framework V2 renders with) resolves its data path to
        // `$XDG_DATA_HOME/opentui`, where its Tree-sitter highlight worker caches
        // parsers and queries. Without this writable mount the worker fails
        // silently and markdown/code highlighting degrades to raw text for
        // non-root runs, so do not remove it (see README "OpenCode V2").
        self.args.push("-v".into());
        self.args.push(format!(
            "{}:/opencode/data/opentui",
            data.join("opentui").display()
        ));
        self.args.push("-v".into());
        self.args.push(format!(
            "{}:/opencode/state/opencode",
            state.join("opencode").display()
        ));
        Ok(self)
    }

    pub fn cargo_home(&mut self) -> Result<&mut Self> {
        let data = dirs::data_dir().context("data dir required")?;
        let _ = std::fs::create_dir_all(data.join("cargo"));
        self.args.push("-e".into());
        self.args.push("CARGO_HOME=/opencode/cargo".into());
        self.args.push("-v".into());
        self.args
            .push(format!("{}:/opencode/cargo", data.join("cargo").display()));
        Ok(self)
    }

    pub fn workspace(&mut self) -> Result<&mut Self> {
        let cwd = std::env::current_dir().context("failed to get current directory")?;
        let basename = cwd
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "root".to_string());

        self.args.push("-v".into());
        self.args
            .push(format!("{}:/workspace/{}", cwd.display(), basename));
        self.args.push("-w".into());
        self.args.push(format!("/workspace/{}", basename));
        Ok(self)
    }

    pub fn security(&mut self) -> &mut Self {
        self.args.push("--security-opt=no-new-privileges".into());
        self.args.push("--cap-drop".into());
        self.args.push("all".into());
        self.args.push("--read-only".into());
        self.args.push("--tmpfs".into());
        self.args.push("/tmp:exec".into());
        self
    }

    pub fn network(&mut self) -> &mut Self {
        self.args.push("--add-host".into());
        self.args.push("host.docker.internal:host-gateway".into());
        self.args.push("-v".into());
        self.args.push("/etc/localtime:/etc/localtime:ro".into());
        self
    }

    pub fn r_library(&mut self) -> Result<&mut Self> {
        let data = dirs::data_dir().context("data dir required")?;
        let r_lib = data.join("opencode-docker/r-library");

        self.args.push("-e".into());
        self.args.push(format!("R_LIBS_SITE={}", r::R_LIB_SITE));
        self.args.push("-v".into());
        self.args
            .push(format!("{}:{}", r_lib.display(), r::R_LIB_SITE));
        Ok(self)
    }

    /// Mount the host R XDG dirs so `tools::R_user_dir()` paths (e.g. renv's
    /// project library, sass's cache) are writable and persist across runs.
    pub fn r_user_dirs(&mut self) -> Result<&mut Self> {
        for (host, target) in r::r_user_dir_mounts()? {
            self.args.push("-v".into());
            self.args.push(format!("{}:{}", host.display(), target));
        }
        Ok(self)
    }

    pub fn git_config(&mut self) -> Result<&mut Self> {
        let home = dirs::home_dir().context("home directory required")?;
        let git_config = home.join(".config/git");
        if git_config.is_dir() {
            self.args.push("-v".into());
            self.args
                .push(format!("{}:/opencode/config/git:ro", git_config.display()));
        }
        Ok(self)
    }

    pub fn ports(&mut self, ports: &[u16]) -> &mut Self {
        for port in ports {
            self.args.push("-p".into());
            self.args.push(format!("127.0.0.1:{}:{}", port, port));
        }
        self
    }

    pub fn extra_mounts(&mut self, mounts: &[String]) -> Result<&mut Self> {
        let cwd =
            std::env::current_dir().context("failed to get current directory for extra mounts")?;
        let cwd_basename = cwd
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "root".to_string());

        let mut seen_targets = HashSet::from([cwd_basename]);

        for mount_spec in mounts {
            let (host_path, target_name) = if let Some(idx) = mount_spec.find(':') {
                let host = &mount_spec[..idx];
                let target = &mount_spec[idx + 1..];
                (host, Some(target.to_string()))
            } else {
                (mount_spec.as_str(), None)
            };

            let resolved = path::resolve_path(host_path)
                .with_context(|| format!("failed to resolve mount path '{}'", host_path))?;

            let target = target_name.unwrap_or_else(|| {
                resolved
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .expect("mount path has no filename")
            });

            if !seen_targets.insert(target.clone()) {
                anyhow::bail!("duplicate mount target '/workspace/{}'", target);
            }

            self.args.push("-v".into());
            self.args
                .push(format!("{}:/workspace/{}", resolved.display(), target));
        }
        Ok(self)
    }

    pub fn audio(&mut self, enabled: bool) -> Result<&mut Self> {
        if !enabled {
            return Ok(self);
        }
        let runtime_dir = Path::new("/run/user").join(util::uid());
        let pulse_socket = runtime_dir.join("pulse/native");
        if !pulse_socket.exists() {
            eprintln!(
                "Warning: PulseAudio socket not found at {}; skipping audio setup",
                pulse_socket.display()
            );
            return Ok(self);
        }

        let pulse_dir = pulse_socket
            .parent()
            .context("pulse socket has no parent")?;
        self.args.push("-e".into());
        self.args
            .push(format!("PULSE_SERVER=unix:{}", pulse_socket.display()));
        self.args.push("-v".into());
        self.args.push(format!(
            "{}:{}:ro",
            pulse_dir.display(),
            pulse_dir.display()
        ));

        let home = dirs::home_dir().context("home directory required")?;
        let cookie = home.join(".config/pulse/cookie");
        if cookie.exists() {
            self.args.push("-e".into());
            self.args.push("PULSE_COOKIE=/opencode/pulse-cookie".into());
            self.args.push("-v".into());
            self.args
                .push(format!("{}:/opencode/pulse-cookie:rw", cookie.display()));
        }
        Ok(self)
    }

    pub fn env_vars(&mut self, vars: &HashMap<String, String>) -> &mut Self {
        for (k, v) in vars {
            self.args.push("-e".into());
            self.args.push(format!("{}={}", k, v));
        }
        self
    }
}

/// Build the host-side read-only mounts for the global opencode config
/// files, creating the default config on first run if none exists yet.
///
/// opencode merges every existing global config file (`opencode.jsonc`,
/// `opencode.json`, `config.json` — jsonc winning on conflicts), so *all* of
/// them are mounted read-only; leaving one writable would defeat the "never
/// rewrite your own permissions" guarantee. Returns an empty vector (with a
/// warning) when no config file could be created.
fn opencode_config_mounts(config_dir: &Path) -> Result<Vec<String>> {
    let mut mounts = Vec::new();
    for name in ["opencode.jsonc", "opencode.json", "config.json"] {
        let path = config_dir.join(name);
        if path.is_file() {
            mounts.push(format!(
                "{}:/opencode/config/opencode/{}:ro",
                path.display(),
                name
            ));
        }
    }
    if mounts.is_empty() {
        // No global config yet: seed the default so a locked run still has
        // something to protect (and mount it).
        if let Some(file) = seed_opencode_config(config_dir) {
            let name = file
                .file_name()
                .context("config file has no filename")?
                .to_string_lossy();
            mounts.push(format!(
                "{}:/opencode/config/opencode/{}:ro",
                file.display(),
                name
            ));
        }
    }
    Ok(mounts)
}

/// Push a read-only mount for `dir` as `host:/opencode/config/opencode/<name>:ro`,
/// skipping (with a warning) when the host-side dir does not exist: mounting a
/// nonexistent source would make Docker create a root-owned directory in the
/// user's config tree.
fn push_ro_dir_mount(da: &mut DockerArgs, dir: &Path, name: &str) {
    if !dir.is_dir() {
        eprintln!(
            "Warning: '{}' is missing; skipping the read-only mount for {name}.",
            dir.display()
        );
        return;
    }
    da.args.push("-v".into());
    da.args.push(format!(
        "{}:/opencode/config/opencode/{}:ro",
        dir.display(),
        name
    ));
}

/// Read-only mounts (`host:/opencode/config/opencode/<name>:ro`) for every
/// existing file among `names` in `config_dir`, e.g. the npm package
/// manifests. Missing files are skipped: there is nothing to protect, and
/// mounting a nonexistent source would create a shadowing directory.
fn existing_file_mounts(config_dir: &Path, names: &[&str]) -> Vec<String> {
    names
        .iter()
        .filter(|name| config_dir.join(name).is_file())
        .map(|name| {
            format!(
                "{}:/opencode/config/opencode/{}:ro",
                config_dir.join(name).display(),
                name
            )
        })
        .collect()
}

/// Seed the default opencode config if none exists, logging a warning when
/// that fails (e.g. because the config dir is not writable): the run then
/// simply proceeds without a config file to protect.
fn seed_opencode_config(config_dir: &Path) -> Option<PathBuf> {
    match ensure_opencode_config(config_dir) {
        Ok(Some(file)) => Some(file),
        Ok(None) => None,
        Err(err) => {
            eprintln!(
                "Warning: could not seed a default opencode config in '{}' ({err:#}); running without one.",
                config_dir.display()
            );
            None
        }
    }
}

/// Locate an existing opencode config file in `config_dir`, seeding the
/// default template when none of the recognized names exists.
///
/// Existing configs are never rewritten: the template is only a first-run
/// default (see `docker/opencode.jsonc.template`). A directory or broken
/// symlink squatting on a recognized name is left untouched and blocks
/// seeding rather than being clobbered.
fn ensure_opencode_config(config_dir: &Path) -> Result<Option<PathBuf>> {
    for name in ["opencode.jsonc", "opencode.json", "config.json"] {
        let path = config_dir.join(name);
        if let Ok(meta) = fs::symlink_metadata(&path) {
            if meta.file_type().is_file() || meta.file_type().is_symlink() {
                // A regular file, or a symlink to one (the bind mount
                // resolves valid symlinks). Broken symlinks report is_file()
                // false and fall through to the warning below.
                if path.is_file() {
                    return Ok(Some(path));
                }
            }
            eprintln!(
                "Warning: '{}' is not a readable config file; leaving it untouched and skipping config seeding.",
                path.display()
            );
            return Ok(None);
        }
    }
    let default = config_dir.join("opencode.jsonc");
    fs::create_dir_all(config_dir).with_context(|| {
        format!(
            "failed to create opencode config directory '{}'",
            config_dir.display()
        )
    })?;
    // Write to a uniquely named temp file and rename into place, so
    // concurrent first runs never observe (or publish) a torn file.
    let mut tmp = tempfile::NamedTempFile::new_in(config_dir)
        .with_context(|| format!("failed to create a temp file in '{}'", config_dir.display()))?;
    tmp.write_all(DEFAULT_OPENCODE_CONFIG.as_bytes())
        .with_context(|| {
            format!(
                "failed to write default opencode config to '{}'",
                tmp.path().display()
            )
        })?;
    if let Err(err) = tmp.persist(&default) {
        // A concurrent first run may have published the config already;
        // treat that as success rather than failing the run.
        if default.is_file() {
            return Ok(Some(default));
        }
        return Err(err).with_context(|| {
            format!(
                "failed to move default opencode config into place at '{}'",
                default.display()
            )
        });
    }
    Ok(Some(default))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_args_empty() {
        let da = DockerArgs::new();
        assert!(da.into_vec().is_empty());
    }

    #[test]
    fn builder_ports_and_env_vars() {
        let mut da = DockerArgs::new();
        da.ports(&[3000, 4000]);
        let mut vars = HashMap::new();
        vars.insert("KEY".to_string(), "val".to_string());
        da.env_vars(&vars);
        let args = da.into_vec();
        assert_eq!(
            args,
            &[
                "-p",
                "127.0.0.1:3000:3000",
                "-p",
                "127.0.0.1:4000:4000",
                "-e",
                "KEY=val",
            ]
        );
    }

    #[test]
    fn builder_security_network() {
        let mut da = DockerArgs::new();
        da.security().network();
        let args = da.into_vec();
        assert!(args.contains(&"--security-opt=no-new-privileges".to_string()));
        assert!(args.contains(&"--cap-drop".to_string()));
        assert!(args.contains(&"all".to_string()));
        assert!(args.contains(&"--read-only".to_string()));
        assert!(args.contains(&"--tmpfs".to_string()));
        assert!(args.contains(&"/tmp:exec".to_string()));
        assert!(args.contains(&"host.docker.internal:host-gateway".to_string()));
        assert!(args.contains(&"/etc/localtime:/etc/localtime:ro".to_string()));
    }

    #[test]
    fn xdg_dirs_mounts_opentui_data_path() {
        // The TUI's Tree-sitter highlight worker caches under `$XDG_DATA_HOME/opentui`;
        // without this writable mount the worker fails silently and markdown/code
        // highlighting degrades to raw text for non-root runs.
        let tmp = tempfile::tempdir().unwrap();
        let mut da = DockerArgs::new();
        da.xdg_dirs(tmp.path(), false).unwrap();
        let args = da.into_vec();
        assert!(
            args.iter().any(|a| a.ends_with(":/opencode/data/opentui")),
            "opentui data-path mount missing: {args:?}"
        );
    }

    #[test]
    fn xdg_dirs_mounts_plugin_surface_readonly() {
        // opencode auto-discovers plugins under the global config dir, and the
        // npm plugin surface (node_modules + manifests) is installed there, so
        // both must be read-only for agents in locked mode.
        let tmp = tempfile::tempdir().unwrap();
        let mut da = DockerArgs::new();
        da.xdg_dirs(tmp.path(), false).unwrap();
        let args = da.into_vec();
        assert!(
            args.iter()
                .any(|a| a.ends_with(":/opencode/config/opencode/plugins:ro")),
            "plugins ro mount missing: {args:?}"
        );
        assert!(
            args.iter()
                .any(|a| a.ends_with(":/opencode/config/opencode/node_modules:ro")),
            "node_modules ro mount missing: {args:?}"
        );
        // The seeded default config is mounted ro too.
        assert!(
            args.iter()
                .any(|a| a.ends_with(":/opencode/config/opencode/opencode.jsonc:ro")),
            "config file ro mount missing: {args:?}"
        );
    }

    #[test]
    fn xdg_dirs_edit_config_skips_readonly_mounts() {
        // `--edit-config` is the deliberate escape hatch: the config files,
        // plugins dir and npm plugin surface are left writable (no ro mounts)
        // so config and plugins can be edited from inside the container.
        let tmp = tempfile::tempdir().unwrap();
        let mut da = DockerArgs::new();
        da.xdg_dirs(tmp.path(), true).unwrap();
        let args = da.into_vec();
        for forbidden in [
            "opencode.jsonc:ro",
            "opencode.json:ro",
            "config.json:ro",
            "plugins:ro",
            "node_modules:ro",
            "package.json:ro",
        ] {
            assert!(
                !args.iter().any(|a| a.contains(forbidden)),
                "'{forbidden}' mount should be skipped with --edit-config: {args:?}"
            );
        }
    }

    #[test]
    fn builder_chaining() {
        let mut da = DockerArgs::new();
        da.ports(&[8080]).security().network();
        let args = da.into_vec();
        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"127.0.0.1:8080:8080".to_string()));
    }

    #[test]
    fn extra_mounts_rejects_duplicate() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("subdir");
        std::fs::create_dir(&sub).unwrap();
        let mounts = vec![
            sub.to_string_lossy().to_string(),
            tmp.path().join("subdir").to_string_lossy().to_string(),
        ];
        let mut da = DockerArgs::new();
        let result = da.extra_mounts(&mounts);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("duplicate"));
    }

    #[test]
    fn extra_mounts_rejects_nonexistent() {
        let mounts = vec!["/nonexistent_opencode_test_xyz".to_string()];
        let mut da = DockerArgs::new();
        let result = da.extra_mounts(&mounts);
        assert!(result.is_err());
    }

    #[test]
    fn config_mounts_seed_default_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("opencode");
        let mounts = opencode_config_mounts(&dir).unwrap();
        assert_eq!(mounts.len(), 1);
        assert!(mounts[0].ends_with(":/opencode/config/opencode/opencode.jsonc:ro"));
        assert!(mounts[0].starts_with(&dir.display().to_string()));
        let content = fs::read_to_string(dir.join("opencode.jsonc")).unwrap();
        assert!(content.contains("\"permissions\""));
        assert!(content.contains("edit:/opencode/config/opencode/*"));
        assert!(content.contains("\"experimental\""));
    }

    #[test]
    fn config_mounts_keep_existing_jsonc() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("opencode");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("opencode.jsonc"), "{\"model\":\"x\"}").unwrap();
        let mounts = opencode_config_mounts(&dir).unwrap();
        assert_eq!(mounts.len(), 1);
        assert!(mounts[0].ends_with(":/opencode/config/opencode/opencode.jsonc:ro"));
        assert_eq!(
            fs::read_to_string(dir.join("opencode.jsonc")).unwrap(),
            "{\"model\":\"x\"}"
        );
    }

    #[test]
    fn config_mounts_protect_every_existing_config_file() {
        // opencode merges all existing global config files, so all of them
        // must be mounted read-only -- not just the precedence winner.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("opencode");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("opencode.json"), "{\"model\":\"y\"}").unwrap();
        fs::write(dir.join("config.json"), "{\"model\":\"c\"}").unwrap();
        let mounts = opencode_config_mounts(&dir).unwrap();
        assert_eq!(mounts.len(), 2);
        assert!(mounts.iter().any(|m| m.ends_with("opencode.json:ro")));
        assert!(mounts.iter().any(|m| m.ends_with("config.json:ro")));
        assert!(
            !dir.join("opencode.jsonc").exists(),
            "no seeding when a config exists"
        );
        // When a jsonc appears later it is mounted too, on top of the others.
        fs::write(dir.join("opencode.jsonc"), "{\"model\":\"z\"}").unwrap();
        let mounts = opencode_config_mounts(&dir).unwrap();
        assert_eq!(mounts.len(), 3);
        assert!(mounts.iter().any(|m| m.ends_with("opencode.jsonc:ro")));
        assert_eq!(
            fs::read_to_string(dir.join("opencode.json")).unwrap(),
            "{\"model\":\"y\"}"
        );
    }

    #[test]
    fn config_mounts_skip_when_unwritable() {
        // A file where the config directory would be needed makes seeding fail;
        // the mount is skipped instead of failing the run.
        let tmp = tempfile::tempdir().unwrap();
        let blocker = tmp.path().join("opencode");
        fs::write(&blocker, "x").unwrap();
        assert!(opencode_config_mounts(&blocker).unwrap().is_empty());
    }

    #[test]
    fn config_mounts_ignore_directory_shadowing_config() {
        // A directory named like a config file is not a config file; the seed
        // write then fails (the name is taken by a dir), so no mount is
        // produced rather than mounting a directory over the file path.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("opencode");
        fs::create_dir_all(dir.join("opencode.jsonc")).unwrap();
        assert!(opencode_config_mounts(&dir).unwrap().is_empty());
    }

    #[test]
    fn existing_file_mounts_only_existing() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("package.json"), "{}").unwrap();
        fs::write(tmp.path().join("bun.lock"), "").unwrap();
        let mounts = existing_file_mounts(
            tmp.path(),
            &["package.json", "package-lock.json", "bun.lock"],
        );
        assert_eq!(mounts.len(), 2);
        assert!(mounts[0].ends_with(":/opencode/config/opencode/package.json:ro"));
        assert!(mounts[1].ends_with(":/opencode/config/opencode/bun.lock:ro"));
    }
}

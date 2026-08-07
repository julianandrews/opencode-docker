use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};

use crate::path;
use crate::r;
use crate::util;

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

    pub fn xdg_dirs(&mut self) -> Result<&mut Self> {
        let config = dirs::config_dir().context("config dir required")?;
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
            "{}:/opencode/config/opencode:ro",
            config.join("opencode").display()
        ));
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
}

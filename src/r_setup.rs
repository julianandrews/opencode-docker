use std::fs;

use anyhow::{Context, Result};
use digest::Digest;

use crate::util;

pub fn check_and_restore() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let renv_lock = cwd.join("renv.lock");
    if !renv_lock.exists() {
        return Ok(());
    }

    let contents = fs::read_to_string(&renv_lock)
        .with_context(|| format!("failed to read {}", renv_lock.display()))?;
    let hash = format!("{:x}", md5::Md5::digest(contents.as_bytes()));

    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?;
    let r_lib_cache = home.join(".local/share/opencode-docker/r-library");
    let marker = r_lib_cache.join(format!(".setup-{}", hash));

    if marker.exists() {
        return Ok(());
    }

    println!("→ Installing R dependencies (first time or lockfile changed)...");
    fs::create_dir_all(&r_lib_cache)?;

    let uid = util::uid();
    let gid = util::gid();
    let cwd_basename = cwd
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "root".to_string());

    let tmp = tempfile::tempdir()?;
    let r_script_path = tmp.path().join("r-setup.R");
    std::fs::write(&r_script_path, include_str!("../docker/r-setup.R"))?;

    let mut cmd = std::process::Command::new("docker");
    cmd.args([
        "run",
        "--rm",
        "--user",
        &format!("{}:{}", uid, gid),
        "-e",
        "HOME=/tmp",
        "-e",
        "R_LIBS_SITE=/opt/r-library",
        "-e",
        "RENV_PATHS_ROOT=/tmp/renv",
        "-v",
        &format!("{}:/opt/r-library", r_lib_cache.display()),
        "-v",
        &format!("{}:/workspace/{}", cwd.display(), cwd_basename),
        "-v",
        &format!("{}:/setup.R:ro", r_script_path.display()),
        "-w",
        &format!("/workspace/{}", cwd_basename),
        "--tmpfs",
        "/tmp:exec",
        "--read-only",
        "--entrypoint",
        "R",
        "opencode-local",
        "-f",
        "/setup.R",
    ]);

    if let Ok(url) = std::env::var("HF_DB_URL") {
        cmd.args(["-e", &format!("HF_DB_URL={}", url)]);
    }

    let status = cmd.status()
        .context("failed to run R dependency setup")?;
    if !status.success() {
        anyhow::bail!("R dependency setup failed (exit code: {:?})", status.code());
    }

    fs::File::create(&marker)?;

    Ok(())
}

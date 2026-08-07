use std::collections::HashMap;
use std::fs;
use std::io::BufRead;

use anyhow::{Context, Result};
use digest::Digest;

use crate::r;
use crate::util;

pub fn check_and_restore(image: &str, env_vars: &HashMap<String, String>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let renv_lock = cwd.join("renv.lock");
    if !renv_lock.exists() {
        return Ok(());
    }

    let contents = fs::read_to_string(&renv_lock)
        .with_context(|| format!("failed to read {}", renv_lock.display()))?;

    let mut env_pairs: Vec<String> = env_vars.iter().map(|(k, v)| format!("{k}={v}")).collect();
    env_pairs.sort();
    let image_id = image_id(image)?;
    let key = format!("{}\n{}\n{}", contents, env_pairs.join("\n"), image_id);
    let hash = format!("{:x}", md5::Md5::digest(key.as_bytes()));

    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?;
    let r_lib_cache = home.join(".local/share/opencode-docker/r-library");
    let marker = r_lib_cache.join(format!(".setup-{}", hash));

    if marker.exists() {
        return Ok(());
    }

    println!("→ Installing R dependencies (lockfile, env, or image changed)...");
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
        &format!("R_LIBS_SITE={}", r::R_LIB_SITE),
        "-e",
        &format!("R_REPOSITORIES={}", r::R_CRAN_REPO),
    ]);

    for (name, value) in r::XDG_ENV {
        cmd.args(["-e", &format!("{}={}", name, value)]);
    }

    for (k, v) in env_vars {
        cmd.args(["-e", &format!("{}={}", k, v)]);
    }

    for (host, target) in r::r_user_dir_mounts()? {
        cmd.args(["-v", &format!("{}:{}", host.display(), target)]);
    }

    cmd.args([
        "-v",
        &format!("{}:{}", r_lib_cache.display(), r::R_LIB_SITE),
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
        image,
        "-f",
        "/setup.R",
    ]);

    let status = cmd.status().context("failed to run R dependency setup")?;
    if !status.success() {
        anyhow::bail!("R dependency setup failed (exit code: {:?})", status.code());
    }

    fs::File::create(&marker)?;

    Ok(())
}

fn image_id(name: &str) -> Result<String> {
    let output = std::process::Command::new("docker")
        .args(["image", "ls", "--format", "{{.ID}}", name])
        .output()
        .context("failed to run docker image ls")?;
    if !output.status.success() {
        anyhow::bail!(
            "docker image ls failed (exit code: {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let id = output
        .stdout
        .lines()
        .next()
        .transpose()?
        .map(|l| l.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("image '{}' not found", name))?;
    Ok(id)
}

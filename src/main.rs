use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use clap::Parser;

mod config;
mod docker;
mod path;
mod r_setup;
mod util;

use config::Feature;

#[derive(Parser)]
#[command(
    name = "opencode-docker",
    version,
    about = "Run opencode in a Docker container"
)]
struct Cli {
    #[arg(short = 'p', long = "profile")]
    profile: Option<String>,

    #[arg(short = 'P', long = "port")]
    port: Vec<u16>,

    #[arg(short = 'm', long = "mount")]
    mount: Vec<String>,

    #[arg(short = 'a', long = "audio")]
    audio: Option<bool>,

    #[arg(short = 'f', long = "dockerfile")]
    dockerfile: Option<PathBuf>,

    #[arg(short = 'e', long = "env")]
    env: Vec<String>,

    #[arg(
        short = 'F',
        long = "feature",
        value_parser = clap::value_parser!(Feature)
    )]
    feature: Vec<Feature>,

    #[arg(long = "no-default-features")]
    no_default_features: bool,

    #[arg(short = 'L', long = "list-profiles", conflicts_with_all = &["profile", "port", "mount", "audio", "dockerfile", "env", "feature", "no_default_features"])]
    list_profiles: bool,

    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.list_profiles {
        let profiles = config::list_profiles().context("failed to load profiles")?;
        if profiles.is_empty() {
            println!("No profiles configured.");
        } else {
            println!("Available profiles:");
            for name in profiles {
                println!("  {name}");
            }
        }
        return Ok(());
    }

    let profile = match cli.profile.as_deref() {
        Some(name) => config::get_profile(name)
            .with_context(|| format!("failed to load config for profile '{name}'"))?,
        None => None,
    };

    let dockerfile = cli
        .dockerfile
        .or_else(|| profile.as_ref().and_then(|p| p.dockerfile.clone()));

    let mut env_vars = profile
        .as_ref()
        .and_then(|p| p.env.clone())
        .unwrap_or_default();
    for e in &cli.env {
        if let Some((k, v)) = e.split_once('=') {
            env_vars.insert(k.to_string(), v.to_string());
        }
    }

    let mut ports = profile
        .as_ref()
        .and_then(|p| p.ports.clone())
        .unwrap_or_default();
    ports.extend(cli.port);

    let mut mounts = profile
        .as_ref()
        .and_then(|p| p.mounts.clone())
        .unwrap_or_default();
    mounts.extend(cli.mount);

    let audio = cli
        .audio
        .or_else(|| profile.as_ref().and_then(|p| p.audio))
        .unwrap_or(true);

    let config_defaults =
        config::get_default_features().context("failed to load default features")?;
    let effective_default = if cli.no_default_features {
        Vec::new()
    } else {
        config_defaults
    };

    let mut features = effective_default;
    if let Some(pf) = profile.as_ref().and_then(|p| p.features.clone()) {
        features.extend(pf);
    }
    features.extend(cli.feature);

    let has_r = features.contains(&Feature::R);
    let has_rust = features.contains(&Feature::Rust);

    check_cwd()?;
    build_image(dockerfile.as_deref(), has_r, has_rust)?;

    if has_r {
        r_setup::check_and_restore()?;
    }

    let mut da = docker::DockerArgs::new();
    da.base()
        .xdg_dirs()
        .workspace()
        .security()
        .network()
        .git_config()
        .ports(&ports)
        .extra_mounts(&mounts)?
        .audio(audio)
        .env_vars(&env_vars)
        .hf_db_url();

    if has_r {
        da.r_library();
    }
    if has_rust {
        da.cargo_home();
    }

    let docker_args = da.into_vec();

    let mut opencode_args = vec!["--hostname".to_string(), "0.0.0.0".to_string()];
    if let Some(&port) = ports.first() {
        opencode_args.push("--port".to_string());
        opencode_args.push(port.to_string());
    }
    opencode_args.extend(cli.args);

    let status = std::process::Command::new("docker")
        .arg("run")
        .args(&docker_args)
        .arg("opencode-local")
        .args(&opencode_args)
        .status()
        .context("failed to execute docker")?;

    std::process::exit(status.code().unwrap_or(1));
}

fn check_cwd() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let home = dirs::home_dir().ok_or_else(|| anyhow!("cannot determine home directory"))?;

    if cwd == home {
        eprintln!("\x1b[33mWarning: Current directory is your home directory ($HOME).\x1b[0m");
        eprintln!("This would mount your entire home directory into the container.");
        print!("Proceed anyway? [y/N] ");
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            std::process::exit(1);
        }
    } else if !cwd.starts_with(&home) {
        eprintln!("\x1b[33mWarning: Current directory is outside your home directory.\x1b[0m");
        eprintln!("Mounting '{}' into the container.", cwd.display());
        print!("Proceed anyway? [y/N] ");
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            std::process::exit(1);
        }
    }

    Ok(())
}

fn build_image(dockerfile: Option<&Path>, has_r: bool, has_rust: bool) -> Result<()> {
    let mut cmd = std::process::Command::new("docker");
    cmd.args(["build", "--pull", "-t", "opencode-local"]);

    if has_r {
        cmd.args(["--build-arg", "INCLUDE_R=true"]);
    }
    cmd.args(["--build-arg", &format!("INCLUDE_RUST={has_rust}")]);

    let tmp;
    if let Some(path) = dockerfile {
        let dir = path
            .parent()
            .ok_or_else(|| anyhow!("dockerfile has no parent directory"))?;
        cmd.arg("-f").arg(path).arg(dir);
    } else {
        tmp = tempfile::tempdir()?;
        let df = tmp.path().join("Dockerfile");
        std::fs::write(&df, include_str!("../docker/Dockerfile"))?;
        cmd.arg(tmp.path());
    }

    let status = cmd.status().context("failed to run docker build")?;
    if !status.success() {
        anyhow::bail!("docker build failed (exit code: {:?})", status.code());
    }
    Ok(())
}

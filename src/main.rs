use std::path::PathBuf;

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

    #[arg(short = 'C', long = "build-context")]
    build_context: Option<PathBuf>,

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

    let cfg = config::resolve(&config::ResolveOptions {
        profile: cli.profile,
        env: cli.env,
        ports: cli.port,
        mounts: cli.mount,
        audio: cli.audio,
        dockerfile: cli.dockerfile,
        build_context: cli.build_context,
        features: cli.feature,
        no_default_features: cli.no_default_features,
    })?;

    check_cwd()?;
    build_image(cfg.dockerfile.as_ref(), &cfg.features)?;

    if cfg.features.contains(&Feature::R) {
        r_setup::check_and_restore()?;
    }

    run_container(&cfg, &cli.args)
}

fn run_container(cfg: &config::ResolvedConfig, extra_args: &[String]) -> Result<()> {
    let mut da = docker::DockerArgs::new();
    da.base()
        .xdg_dirs()?
        .workspace()?
        .security()
        .network()
        .git_config()?
        .ports(&cfg.ports)
        .extra_mounts(&cfg.mounts)?
        .audio(cfg.audio)?
        .env_vars(&cfg.env_vars)
        .hf_db_url();

    if cfg.features.contains(&Feature::R) {
        da.r_library()?;
    }
    if cfg.features.contains(&Feature::Rust) {
        da.cargo_home()?;
    }

    let docker_args = da.into_vec();

    let mut opencode_args = vec!["--hostname".to_string(), "0.0.0.0".to_string()];
    if let Some(&port) = cfg.ports.first() {
        opencode_args.push("--port".to_string());
        opencode_args.push(port.to_string());
    }
    opencode_args.extend(extra_args.iter().cloned());

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
        confirm_or_exit(&[
            "\x1b[33mWarning: Current directory is your home directory ($HOME).\x1b[0m".into(),
            "This would mount your entire home directory into the container.".into(),
        ])?;
    } else if !cwd.starts_with(&home) {
        confirm_or_exit(&[
            "\x1b[33mWarning: Current directory is outside your home directory.\x1b[0m".into(),
            format!("Mounting '{}' into the container.", cwd.display()),
        ])?;
    }

    Ok(())
}

fn confirm_or_exit(lines: &[String]) -> Result<()> {
    for line in lines {
        eprintln!("{}", line);
    }
    print!("Proceed anyway? [y/N] ");
    std::io::Write::flush(&mut std::io::stdout())?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if !input.trim().eq_ignore_ascii_case("y") {
        std::process::exit(1);
    }
    Ok(())
}

fn build_image(
    resolved: Option<&config::ResolvedDockerfile>,
    features: &[Feature],
) -> Result<()> {
    let has_r = features.contains(&Feature::R);
    let has_rust = features.contains(&Feature::Rust);
    let status = {
        let tmp = tempfile::tempdir()?;
        let df = tmp.path().join("Dockerfile");
        std::fs::write(&df, include_str!("../docker/Dockerfile"))?;
        let mut cmd = std::process::Command::new("docker");
        cmd.args(["build", "--pull", "-t", "opencode-local-base"]);
        if has_r {
            cmd.args(["--build-arg", "INCLUDE_R=true"]);
        }
        cmd.args(["--build-arg", &format!("INCLUDE_RUST={has_rust}")]);
        cmd.arg(tmp.path());
        cmd.status().context("failed to run docker build (base)")?
    };
    if !status.success() {
        anyhow::bail!(
            "docker build (base) failed (exit code: {:?})",
            status.code()
        );
    }

    if let Some(r) = resolved {
        let mut cmd = std::process::Command::new("docker");
        cmd.args(["build", "-t", "opencode-local"]);
        if has_r {
            cmd.args(["--build-arg", "INCLUDE_R=true"]);
        }
        cmd.args(["--build-arg", &format!("INCLUDE_RUST={has_rust}")]);
        cmd.arg("-f").arg(&r.dockerfile).arg(&r.build_context);

        let status = cmd
            .status()
            .context("failed to run docker build (custom)")?;
        if !status.success() {
            anyhow::bail!(
                "docker build (custom) failed (exit code: {:?})",
                status.code()
            );
        }
    } else {
        let status = std::process::Command::new("docker")
            .args(["tag", "opencode-local-base", "opencode-local"])
            .status()
            .context("failed to tag base image as opencode-local")?;
        if !status.success() {
            anyhow::bail!(
                "docker tag failed (exit code: {:?})",
                status.code()
            );
        }
    }

    Ok(())
}

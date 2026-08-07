use anyhow::{anyhow, Context, Result};
use clap::Parser;

mod config;
mod docker;
mod path;
mod r;
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

    #[arg(short = 'L', long = "list-profiles", conflicts_with_all = &["profile", "port", "mount", "audio", "env", "feature", "no_default_features", "build"])]
    list_profiles: bool,

    #[arg(
        long = "build",
        help = "Build the configured feature variants and exit (used by the systemd timer)",
        conflicts_with_all = &["profile", "port", "mount", "audio", "env", "no_default_features"]
    )]
    build: bool,

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

    if cli.build {
        if !cli.args.is_empty() {
            anyhow::bail!("unexpected arguments with --build: {:?}", cli.args);
        }
        return cmd_build(&cli);
    }

    let cfg = config::resolve(&config::ResolveOptions {
        profile: cli.profile,
        env: cli.env,
        ports: cli.port,
        mounts: cli.mount,
        audio: cli.audio,
        features: cli.feature,
        no_default_features: cli.no_default_features,
    })?;

    check_cwd()?;

    let image = resolve_image(&cfg)?;

    if cfg.features.contains(&Feature::R) {
        r_setup::check_and_restore(&image, &cfg.env_vars)?;
    }

    run_container(&cfg, &image, &cli.args)
}

fn resolve_image(cfg: &config::ResolvedConfig) -> Result<String> {
    let image = config::image_name_for(&cfg.features);
    if config::variants_contain(&cfg.build, &cfg.default_features, &cfg.features) {
        ensure_image(&image, &cfg.features)?;
    } else {
        eprintln!(
            "Feature set {image} is not in the [build] variants; rebuilding on every run (add it there to make runs instant)."
        );
        build_variant(&cfg.features, &image, true)?;
    }
    Ok(image)
}

fn run_container(cfg: &config::ResolvedConfig, image: &str, extra_args: &[String]) -> Result<()> {
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
        .env_vars(&cfg.env_vars);

    if cfg.features.contains(&Feature::R) {
        da.r_library()?.r_user_dirs()?;
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
        .arg(image)
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

fn cmd_build(cli: &Cli) -> Result<()> {
    let variants = if cli.feature.is_empty() {
        let cfg = config::load_config()?;
        config::default_variants(&cfg.build, &cfg.default_features)
    } else {
        vec![cli.feature.clone()]
    };

    let mut seen = std::collections::HashSet::new();
    for features in &variants {
        let name = config::image_name_for(features);
        if !seen.insert(name.clone()) {
            continue;
        }
        let label = if features.is_empty() {
            "no features".to_string()
        } else {
            features
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join("+")
        };
        println!("Building {name} ({label})");
        build_variant(features, &name, true)?;
    }
    prune_images()?;
    Ok(())
}

fn prune_images() -> Result<()> {
    let prune = std::process::Command::new("docker")
        .args(["image", "prune", "-f"])
        .status()
        .context("failed to run docker image prune")?;
    if !prune.success() {
        anyhow::bail!("docker image prune failed (exit code: {:?})", prune.code());
    }
    let cache = std::process::Command::new("docker")
        .args(["builder", "prune", "--keep-storage=10G"])
        .status()
        .context("failed to run docker builder prune")?;
    if !cache.success() {
        anyhow::bail!(
            "docker builder prune failed (exit code: {:?})",
            cache.code()
        );
    }
    Ok(())
}

/// Check whether an image exists locally.
///
/// Deliberately uses `docker image ls` (exit 0, empty output on a miss)
/// rather than `docker image inspect`'s exit code: both "image not found" and
/// infrastructure failures (daemon down, CLI error) exit non-zero, so an
/// exit-code-only check would silently treat real errors as a missing image.
/// Here a non-zero exit is a genuine error and is propagated, while exit 0
/// with empty output definitively means "not found".
///
/// The positional reference matches the repository exactly but any tag of it;
/// that is fine because every image is built with the implicit `:latest` tag.
fn image_exists(name: &str) -> Result<bool> {
    let output = std::process::Command::new("docker")
        .args(["image", "ls", "--format", "{{.Repository}}:{{.Tag}}", name])
        .output()
        .context("failed to run docker image ls")?;
    if !output.status.success() {
        anyhow::bail!(
            "docker image ls failed (exit code: {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(!output.stdout.is_empty())
}

fn ensure_image(name: &str, features: &[Feature]) -> Result<()> {
    if image_exists(name)? {
        return Ok(());
    }
    eprintln!(
        "Image '{name}' not found locally; building it now (pre-build it with `opencode-docker --build` or the systemd timer to keep startups instant)."
    );
    build_variant(features, name, false)
}

fn build_variant(features: &[Feature], tag: &str, pull: bool) -> Result<()> {
    let has_r = features.contains(&Feature::R);
    let has_rust = features.contains(&Feature::Rust);
    let tmp = tempfile::tempdir()?;
    let df = tmp.path().join("Dockerfile");
    std::fs::write(&df, include_str!("../docker/Dockerfile"))?;
    let mut cmd = std::process::Command::new("docker");
    cmd.args(["build", "-t", tag]);
    if pull {
        cmd.arg("--pull");
    }
    if has_r {
        cmd.args(["--build-arg", "INCLUDE_R=true"]);
    }
    cmd.args(["--build-arg", &format!("INCLUDE_RUST={has_rust}")]);
    cmd.arg(tmp.path());
    let status = cmd.status().context("failed to run docker build")?;
    if !status.success() {
        anyhow::bail!("docker build failed (exit code: {:?})", status.code());
    }
    Ok(())
}

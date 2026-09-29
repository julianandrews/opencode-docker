use anyhow::{anyhow, Context, Result};
use clap::Parser;

mod config;
mod docker;
mod path;
mod r;
mod r_setup;
mod registry;
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
        ensure_image(&image, &cfg.features, &cfg.build.base_image())?;
    } else {
        eprintln!(
            "Feature set {image} is not in the [build] variants; rebuilding on every run (add it there to make runs instant)."
        );
        build_variant(&cfg.features, &image, &cfg.build.base_image(), true)?;
    }
    Ok(image)
}

fn run_container(cfg: &config::ResolvedConfig, image: &str, extra_args: &[String]) -> Result<()> {
    let subcommand = extra_args.first().map(String::as_str);
    let mut da = docker::DockerArgs::new();
    da.base()
        .xdg_dirs()?
        .workspace()?
        .security()
        .network()
        .git_config()?;
    // The TUI runs a private server over stdio in V2, so `-P` only maps a host
    // port when running `serve`.
    if subcommand == Some("serve") {
        da.ports(&cfg.ports);
    } else if !cfg.ports.is_empty() {
        let ports = cfg
            .ports
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!(
            "Warning: -P {ports} ignored: ports only apply to `serve` (the TUI runs over stdio in V2)."
        );
    }
    da.extra_mounts(&cfg.mounts)?
        .audio(cfg.audio)?
        .env_vars(&cfg.env_vars);

    if cfg.features.contains(&Feature::R) {
        da.r_library()?.r_user_dirs()?;
    }
    if cfg.features.contains(&Feature::Rust) {
        da.cargo_home()?;
    }

    let docker_args = da.into_vec();

    let opencode_args = opencode_command(cfg.ports.first().copied(), extra_args);

    let status = std::process::Command::new("docker")
        .arg("run")
        .args(&docker_args)
        .arg(image)
        .args(&opencode_args)
        .status()
        .context("failed to execute docker")?;

    std::process::exit(status.code().unwrap_or(1));
}

/// Build the `opencode` argument vector for a run.
///
/// V2 is subcommand-based with a strict root: the TUI (default command) runs a
/// private server over stdio via `--standalone`; `serve` takes the network
/// flags after the subcommand; `run` also runs standalone.
fn opencode_command(port: Option<u16>, extra_args: &[String]) -> Vec<String> {
    let subcommand = extra_args.first().map(String::as_str);
    let mut args = Vec::new();

    match subcommand {
        Some("serve") => {
            args.push("serve".to_string());
            push_hostname_port(&mut args, port);
            args.extend(extra_args.iter().skip(1).cloned());
        }
        Some("run") => {
            args.push("run".to_string());
            push_standalone(&mut args, &extra_args[1..]);
            args.extend(extra_args.iter().skip(1).cloned());
        }
        _ => {
            push_standalone(&mut args, extra_args);
            args.extend(extra_args.iter().cloned());
        }
    }
    args
}

fn push_hostname_port(args: &mut Vec<String>, port: Option<u16>) {
    args.push("--hostname".to_string());
    args.push("0.0.0.0".to_string());
    if let Some(port) = port {
        args.push("--port".to_string());
        args.push(port.to_string());
    }
}

/// Add `--standalone` unless the user already picked a connection mode.
fn push_standalone(args: &mut Vec<String>, rest: &[String]) {
    let explicit = rest.iter().any(|a| a == "--standalone" || a == "--server");
    if !explicit {
        args.push("--standalone".to_string());
    }
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
    let cfg = config::load_config()?;
    let variants = if cli.feature.is_empty() {
        config::default_variants(&cfg.build, &cfg.default_features)
    } else {
        vec![cli.feature.clone()]
    };

    let base_image = build_base_image(&cfg);

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
        build_variant(features, &name, &base_image, true)?;
    }
    prune_images()?;
    Ok(())
}

/// The base image for a `--build` run: the configured pin, or the newest
/// OpenCode V2 release tag from the registry, falling back to the default
/// when the registry can't be reached (the daily timer must not hard-fail).
fn build_base_image(cfg: &config::Config) -> String {
    build_base_image_with(cfg, registry::latest_base_image)
}

fn build_base_image_with(
    cfg: &config::Config,
    discover: impl Fn() -> Result<registry::Latest>,
) -> String {
    if let Some(pin) = &cfg.build.base_image {
        println!("Using pinned base image {pin}");
        return pin.clone();
    }
    match discover() {
        Ok(latest) => {
            if let Some(image) = latest.image {
                println!("Newest OpenCode V2 release: {image}");
                image
            } else {
                let reason = if latest.truncated {
                    "registry tag list was cut off before it ended"
                } else {
                    "registry has no 2.x release tags"
                };
                eprintln!(
                    "Warning: {reason}; using default {}. Pin one with [build] base_image.",
                    config::DEFAULT_BASE_IMAGE
                );
                config::DEFAULT_BASE_IMAGE.to_string()
            }
        }
        Err(err) => {
            eprintln!(
                "Warning: could not check the registry for the newest OpenCode V2 release ({err:#}); using default {}. Pin one with [build] base_image.",
                config::DEFAULT_BASE_IMAGE
            );
            config::DEFAULT_BASE_IMAGE.to_string()
        }
    }
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

fn ensure_image(name: &str, features: &[Feature], base_image: &str) -> Result<()> {
    if image_exists(name)? {
        return Ok(());
    }
    eprintln!(
        "Image '{name}' not found locally; building it now (pre-build it with `opencode-docker --build` or the systemd timer to keep startups instant)."
    );
    build_variant(features, name, base_image, false)
}

fn build_variant(features: &[Feature], tag: &str, base_image: &str, pull: bool) -> Result<()> {
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
    cmd.args(["--build-arg", &format!("BASE_IMAGE={base_image}")]);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn tui_default_gets_standalone() {
        assert_eq!(opencode_command(None, &s(&[])), s(&["--standalone"]));
        assert_eq!(
            opencode_command(Some(8080), &s(&["--model", "foo/bar"])),
            s(&["--standalone", "--model", "foo/bar"])
        );
    }

    #[test]
    fn tui_with_server_skips_standalone() {
        assert_eq!(
            opencode_command(None, &s(&["--server", "http://localhost:4096"])),
            s(&["--server", "http://localhost:4096"])
        );
        assert_eq!(
            opencode_command(None, &s(&["--standalone", "--model", "foo/bar"])),
            s(&["--standalone", "--model", "foo/bar"])
        );
    }

    #[test]
    fn serve_places_network_flags_after_subcommand() {
        assert_eq!(
            opencode_command(Some(8080), &s(&["serve"])),
            s(&["serve", "--hostname", "0.0.0.0", "--port", "8080"])
        );
        assert_eq!(
            opencode_command(None, &s(&["serve"])),
            s(&["serve", "--hostname", "0.0.0.0"])
        );
        assert_eq!(
            opencode_command(Some(8080), &s(&["serve", "--cors", "http://x"])),
            s(&[
                "serve",
                "--hostname",
                "0.0.0.0",
                "--port",
                "8080",
                "--cors",
                "http://x"
            ])
        );
    }

    #[test]
    fn run_gets_standalone() {
        assert_eq!(
            opencode_command(None, &s(&["run", "hello"])),
            s(&["run", "--standalone", "hello"])
        );
        assert_eq!(
            opencode_command(None, &s(&["run", "--server", "http://x", "hello"])),
            s(&["run", "--server", "http://x", "hello"])
        );
    }

    #[test]
    fn build_base_image_uses_pin_without_discovery() {
        let config: config::Config = toml::from_str(
            r#"
[build]
base_image = "ghcr.io/anomalyco/opencode:2.0.18"
"#,
        )
        .unwrap();
        let discovered = std::cell::Cell::new(false);
        let image = build_base_image_with(&config, || {
            discovered.set(true);
            Ok(registry::Latest {
                image: Some("ghcr.io/anomalyco/opencode:9.9.9".to_string()),
                truncated: false,
            })
        });
        assert_eq!(image, "ghcr.io/anomalyco/opencode:2.0.18");
        assert!(!discovered.get());
    }

    #[test]
    fn build_base_image_uses_discovered_when_unset() {
        let config: config::Config = toml::from_str("").unwrap();
        let image = build_base_image_with(&config, || {
            Ok(registry::Latest {
                image: Some("ghcr.io/anomalyco/opencode:2.0.19".to_string()),
                truncated: false,
            })
        });
        assert_eq!(image, "ghcr.io/anomalyco/opencode:2.0.19");
    }

    #[test]
    fn build_base_image_falls_back_when_registry_has_no_v2_tags() {
        let config: config::Config = toml::from_str("").unwrap();
        let image = build_base_image_with(&config, || {
            Ok(registry::Latest {
                image: None,
                truncated: false,
            })
        });
        assert_eq!(image, config::DEFAULT_BASE_IMAGE);
    }

    #[test]
    fn build_base_image_falls_back_when_tag_list_truncated() {
        let config: config::Config = toml::from_str("").unwrap();
        let image = build_base_image_with(&config, || {
            Ok(registry::Latest {
                image: None,
                truncated: true,
            })
        });
        assert_eq!(image, config::DEFAULT_BASE_IMAGE);
    }

    #[test]
    fn build_base_image_falls_back_when_registry_unreachable() {
        let config: config::Config = toml::from_str("").unwrap();
        let image = build_base_image_with(&config, || Err(anyhow!("registry unreachable")));
        assert_eq!(image, config::DEFAULT_BASE_IMAGE);
    }
}

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Feature {
    R,
    Rust,
}

impl FromStr for Feature {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "R" => Ok(Feature::R),
            "Rust" => Ok(Feature::Rust),
            _ => Err(format!("unknown feature '{s}', expected R or Rust")),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub default_features: Vec<Feature>,
    #[serde(default)]
    pub profile: HashMap<String, Profile>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Profile {
    pub env: Option<HashMap<String, String>>,
    pub mounts: Option<Vec<String>>,
    pub ports: Option<Vec<u16>>,
    pub audio: Option<bool>,
    pub dockerfile: Option<PathBuf>,
    pub build_context: Option<PathBuf>,
    pub features: Option<Vec<Feature>>,
}

fn config_path() -> Option<std::path::PathBuf> {
    Some(dirs::config_dir()?.join("opencode-docker/docker-profiles.toml"))
}

fn load_config() -> Result<Config> {
    let path = match config_path() {
        Some(p) => p,
        None => return Ok(Config::default()),
    };
    if !path.exists() {
        return Ok(Config::default());
    }
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("failed to read config file '{}'", path.display()))?;
    let config: Config = toml::from_str(&contents)
        .with_context(|| format!("failed to parse config file '{}'", path.display()))?;
    Ok(config)
}

pub fn list_profiles() -> Result<Vec<String>> {
    let config = load_config()?;
    Ok(config.profile.into_keys().collect())
}

#[derive(Debug, Clone, Default)]
pub struct ResolveOptions {
    pub profile: Option<String>,
    pub env: Vec<String>,
    pub ports: Vec<u16>,
    pub mounts: Vec<String>,
    pub audio: Option<bool>,
    pub dockerfile: Option<PathBuf>,
    pub build_context: Option<PathBuf>,
    pub features: Vec<Feature>,
    pub no_default_features: bool,
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub env_vars: HashMap<String, String>,
    pub ports: Vec<u16>,
    pub mounts: Vec<String>,
    pub audio: bool,
    pub features: Vec<Feature>,
    pub dockerfile: Option<ResolvedDockerfile>,
}

#[derive(Debug, Clone)]
pub struct ResolvedDockerfile {
    pub dockerfile: PathBuf,
    pub build_context: PathBuf,
}

pub fn resolve(opts: &ResolveOptions) -> Result<ResolvedConfig> {
    let config = load_config()?;

    let profile = opts
        .profile
        .as_deref()
        .and_then(|name| config.profile.get(name).cloned());

    let mut env_vars = profile
        .as_ref()
        .and_then(|p| p.env.clone())
        .unwrap_or_default();
    for e in &opts.env {
        if let Some((k, v)) = e.split_once('=') {
            env_vars.insert(k.to_string(), v.to_string());
        }
    }

    let mut ports = profile
        .as_ref()
        .and_then(|p| p.ports.clone())
        .unwrap_or_default();
    ports.extend(&opts.ports);

    let mut mounts = profile
        .as_ref()
        .and_then(|p| p.mounts.clone())
        .unwrap_or_default();
    mounts.extend(opts.mounts.iter().cloned());

    let audio = opts
        .audio
        .or_else(|| profile.as_ref().and_then(|p| p.audio))
        .unwrap_or(true);

    let effective_default = if opts.no_default_features {
        Vec::new()
    } else {
        config.default_features
    };

    let mut features = effective_default;
    if let Some(pf) = profile.as_ref().and_then(|p| p.features.clone()) {
        features.extend(pf);
    }
    features.extend(opts.features.iter().cloned());

    let dockerfile = resolve_dockerfile(
        opts.dockerfile.as_deref(),
        opts.build_context.as_deref(),
        profile.as_ref().and_then(|p| p.dockerfile.as_deref()),
        profile.as_ref().and_then(|p| p.build_context.as_deref()),
    )?;

    Ok(ResolvedConfig {
        env_vars,
        ports,
        mounts,
        audio,
        features,
        dockerfile,
    })
}

fn resolve_dockerfile(
    cli_dockerfile: Option<&Path>,
    cli_build_context: Option<&Path>,
    profile_dockerfile: Option<&Path>,
    profile_build_context: Option<&Path>,
) -> Result<Option<ResolvedDockerfile>> {
    if let Some(df) = cli_dockerfile {
        let ctx = resolve_context_dir(cli_build_context, df)?;
        Ok(Some(ResolvedDockerfile {
            dockerfile: df.to_path_buf(),
            build_context: ctx,
        }))
    } else if let Some(df) = profile_dockerfile {
        let ctx = cli_build_context.or(profile_build_context);
        Ok(Some(resolve_dockerfile_and_context(df, ctx)?))
    } else {
        Ok(None)
    }
}

fn resolve_context_dir(
    build_context: Option<&Path>,
    dockerfile: &Path,
) -> Result<PathBuf> {
    match build_context {
        Some(c) => c
            .canonicalize()
            .with_context(|| format!("failed to canonicalize build context '{}'", c.display())),
        None => dockerfile
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "cannot determine build context: '{}' has no parent directory",
                    dockerfile.display()
                )
            }),
    }
}

pub fn resolve_dockerfile_and_context(
    dockerfile: &Path,
    build_context: Option<&Path>,
) -> Result<ResolvedDockerfile> {
    if dockerfile.is_absolute() {
        return Ok(ResolvedDockerfile {
            dockerfile: dockerfile.to_path_buf(),
            build_context: resolve_context_dir(build_context, dockerfile)?,
        });
    }

    if dockerfile.exists() {
        let abs = dockerfile.canonicalize()?;
        return Ok(ResolvedDockerfile {
            dockerfile: abs.clone(),
            build_context: resolve_context_dir(build_context, &abs)?,
        });
    }

    let data_dir = dirs::data_dir()
        .ok_or_else(|| anyhow::anyhow!("cannot determine data directory"))?;
    let dockerfiles_dir = data_dir.join("opencode-docker/dockerfiles");

    let candidates = [
        dockerfiles_dir.join(dockerfile).join("Dockerfile"),
        dockerfiles_dir.join(format!("{}.dockerfile", dockerfile.display())),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return Ok(ResolvedDockerfile {
                dockerfile: candidate.clone(),
                build_context: resolve_context_dir(build_context, &candidate)?,
            });
        }
    }

    anyhow::bail!(
        "dockerfile '{}' not found (searched: {:?})",
        dockerfile.display(),
        candidates
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_from_str() {
        assert_eq!("R".parse::<Feature>().unwrap(), Feature::R);
        assert_eq!("Rust".parse::<Feature>().unwrap(), Feature::Rust);
        assert!("r".parse::<Feature>().is_err());
        assert!("rust".parse::<Feature>().is_err());
        assert!("Python".parse::<Feature>().is_err());
    }

    #[test]
    fn feature_deserialize() {
        use serde::de::value::StrDeserializer;
        let r = Feature::deserialize(
            StrDeserializer::<serde::de::value::Error>::new("R"),
        ).unwrap();
        assert_eq!(r, Feature::R);
        let rust = Feature::deserialize(
            StrDeserializer::<serde::de::value::Error>::new("Rust"),
        ).unwrap();
        assert_eq!(rust, Feature::Rust);
    }

    #[test]
    fn config_empty_profiles() {
        let config: Config = toml::from_str("").unwrap();
        assert!(config.profile.is_empty());
        assert!(config.default_features.is_empty());
    }

    #[test]
    fn config_with_profile() {
        let toml_str = r#"
default_features = ["R", "Rust"]

[profile.myenv]
env = { FOO = "bar" }
ports = [8080, 9090]
audio = false
features = ["R"]

[profile.other]
mounts = ["~/data"]
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.default_features, vec![Feature::R, Feature::Rust]);
        assert_eq!(config.profile.len(), 2);

        let myenv = &config.profile["myenv"];
        assert_eq!(myenv.env.as_ref().unwrap().get("FOO").unwrap(), "bar");
        assert_eq!(myenv.ports.as_ref().unwrap(), &[8080, 9090]);
        assert_eq!(myenv.audio, Some(false));
        assert_eq!(myenv.features.as_ref().unwrap(), &[Feature::R]);
    }
}

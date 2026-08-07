# opencode-docker

Run [opencode](https://opencode.ai) inside a Docker container with your workspace mounted in, XDG state persisted, and optional feature toolchains (R, Rust).

## How it works

Runs use the pre-built image for your feature set (`opencode-local`, `opencode-local-r`, `opencode-local-rust`,
`opencode-local-r-rust`) when that set is one of the `[build]` variants, so startups are instant. Freshness is
maintained out-of-band:

- `opencode-docker --build` pulls the latest `ghcr.io/anomalyco/opencode` and rebuilds the configured feature
  variants. This is the only command that talks to the registry.
- A systemd timer runs `--build` daily (see below), so variant images are at most ~1 day behind the latest
  release.
- If a variant image is missing locally (fresh install, pruned cache), a one-time build happens automatically on
  the next run, using only local layers — the base image is pulled only if it is missing too, so variant runs
  never talk to the registry.
- Feature sets that are *not* in `[build]` variants are rebuilt on every run with `docker build --pull`, so they
  always track the latest base image at the cost of a registry check per run. The run prints a reminder to add
  the set to `variants` if you use it often.

## Installation

Build the binary and install it:

```sh
cargo build --release
install -m755 target/release/opencode-docker ~/.local/bin/
```

## Configuration

`~/.config/opencode-docker/docker-profiles.toml`:

```toml
# Feature toolchains enabled by default on every run.
default_features = ["R", "Rust"]

# Feature combinations to pre-build with `opencode-docker --build`.
# Each entry becomes one image variant; runs for these are instant.
# If unset, defaults to the no-feature image plus `default_features`.
[build]
variants = [[], ["R"], ["Rust"], ["R", "Rust"]]

[profile.myenv]
env = { FOO = "bar" }
ports = [8080]
```

Image names are canonical: features are deduplicated, sorted (R before Rust), and lowercased, so
`opencode-local-r-rust` is the same whether the features came from the CLI, a profile, or `default_features`.
Feature precedence is "most specific wins": a profile's `features` replace `default_features`, and
`--feature` on the command line replaces both. `--no-default-features` drops `default_features` (a profile
or `--feature` still applies on top).

Profile `env` entries (overridable with `--env K=V`) are passed to the container; they are also forwarded to the
R dependency-setup container, so anything your R scripts need (e.g. `HF_DB_URL`) goes in the profile. The host
environment is not forwarded. `R_REPOSITORIES` is available in the setup container (defaults to CRAN) and can be
overridden through a profile's `env` too.

When a project has a `renv.lock`, dependency setup runs on every launch: `renv::restore()` is a fast no-op when
the library already satisfies the lockfile, so a synced project pays ~1-2s and setup self-heals if the library
drifts. There is no cached setup state that can go stale. When the project activates renv (its `.Rprofile`), the
restore targets the renv-managed project library -- the same library the app and tests use at runtime. If a
project has a `renv.lock` but does not activate renv, setup warns loudly and falls back to the shared site
library (`R_LIBS_SITE`, see below), which the runtime always reads.

The config file is strict: unknown fields anywhere (top level, `[build]`, or a profile) fail to parse instead of
being silently ignored.

With the R feature enabled, R's user directories (`tools::R_user_dir()`) are persisted to the host:
`~/.cache/R` maps to `/opencode/cache/R`, `~/.local/share/R` to `/opencode/data/R`, `~/.config/R`
to `/opencode/config/R`, and `~/.local/state/R` to `/opencode/state/R`. The R dependency-setup
container uses the same XDG dirs, so the renv package cache is shared and warm across setup runs
and opencode sessions. This keeps renv libraries/caches and sass caches writable (the container runs
`--read-only`) and warm across runs. renv's OS-specific path prefix (`RENV_PATHS_PREFIX_AUTO`) keeps
Alpine-container artifacts separate from anything produced by R on the host itself.

The shared site library `~/.local/share/opencode-docker/r-library` maps to `R_LIBS_SITE=/opt/r-library` in both
the setup and runtime containers. It is the persistent library for R projects without renv (in-session package
installs survive across runs, since the container `HOME` is ephemeral) and the fallback target for `renv.lock`
projects that do not activate renv.

## Keeping images fresh with systemd

The timer runs `opencode-docker --build` once a day and rebuilds only what changed (the base `--pull` build is a
cache hit when there is no new release). Install it for your user:

```sh
install -m644 packaging/opencode-docker-build.timer packaging/opencode-docker-build.service \
  ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now opencode-docker-build.timer
systemctl --user list-timers opencode-docker-build.timer
```

Notes:

- The unit runs `%h/.local/bin/opencode-docker`, where `%h` is systemd's specifier for your home directory. If the
  binary is installed elsewhere, adjust the `ExecStart` line in `opencode-docker-build.service`.
- The build runs at idle priority (`Nice=10`, `IOSchedulingClass=idle`) so it never slows down foreground work.
- On headless machines, run `loginctl enable-linger $USER` so user units start at boot.
- `Persistent=true` catches up after the machine was off.
- Manual refresh: `opencode-docker --build` or `opencode-docker --build --feature R` for a single variant.
- After building, `--build` also prunes dangling images (`docker image prune -f`) and keeps the build cache
  capped at 10 GB (`docker builder prune --keep-storage=10G`), so daily rebuilds don't grow disk usage unbounded.

## Usage

```sh
opencode-docker                          # run in the current directory
opencode-docker -p myenv                 # use a profile
opencode-docker --feature Rust           # enable Rust toolchain for this run
opencode-docker -P 8080 serve            # pass remaining args to opencode
```

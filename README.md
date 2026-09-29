# opencode-docker

Run [opencode](https://opencode.ai) inside a Docker container with your workspace mounted in, XDG state persisted, and optional feature toolchains (R, Rust).

## How it works

Runs use the pre-built image for your feature set (`opencode-local`, `opencode-local-r`, `opencode-local-rust`,
`opencode-local-r-rust`) when that set is one of the `[build]` variants, so startups are instant. Freshness is
maintained out-of-band:

- `opencode-docker --build` rebuilds the configured feature variants FROM the newest OpenCode V2 release: when
  `[build] base_image` is unset it queries the registry and uses the newest `2.x` release tag; set `base_image` to
  pin a specific tag instead. This is the only command that queries the registry for the release tag.
- A systemd timer runs `--build` daily (see below), so variant images are at most ~1 day behind the latest OpenCode
  V2 release.
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

# Base image (with tag) the variants are built FROM. Leave unset to
# auto-track the newest OpenCode V2 release on every --build (the daily timer
# then keeps variants current without manual bumps). Set it to pin an exact
# tag, e.g. to freeze on a known-good release. When unset, runs and failed
# registry lookups fall back to the compiled-in default (2.0.19). Note that
# the `latest` tag still tracks the V1 release, so always use an explicit V2
# tag when pinning.
# base_image = "ghcr.io/anomalyco/opencode:2.0.19"

[profile.myenv]
env = { FOO = "bar" }
ports = [8080]
# edit_config = true   # mount the config dir read-write (see Permissions)
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

## OpenCode V2

Runs use the OpenCode V2 release, auto-tracked by `--build` (fallback default `2.0.19`, or pin with `[build] base_image`).
Moving to V2:

- V1 session history is imported into the shared `~/.local/share/opencode/opencode.db` on first V2 run. Make a backup
  of that database before switching if you want a rollback path (V2 is beta and data may be reset).
- Existing `opencode.json(c)` config and `.opencode/` definitions keep working (V2 reads them in-memory). Terminal
  settings are auto-migrated from `tui.json` to `cli.json` on first startup, which is why the
  `~/.config/opencode` mount is now read-write.
- The `~/.local/share/opentui` mount is kept: V2's TUI renders through opentui, which resolves its data path to
  `$XDG_DATA_HOME/opentui` (`/opencode/data/opentui`). Its Tree-sitter highlight worker caches parsers and queries
  there. Removing the mount makes the worker fail silently and markdown/code highlighting degrade to raw text for
  non-root runs.
- The TUI runs `opencode --standalone` (a private server over stdio) instead of V1's `--hostname 0.0.0.0 --port`, which
  V2 does not accept on the default command. `serve` still takes `--hostname`/`--port`, and `run` runs standalone.

## Permissions

Agents run under a locked-down, opinionated permission policy. It lives in the host's `~/.config/opencode/opencode.json(c)`;
there is no project-level config by default. If no opencode config file exists, the first run seeds the default policy
(kept in `docker/opencode.jsonc.template`) host-side before starting the container — existing configs are never rewritten.

What the default policy does:

- **Reads are quiet for system paths.** The container rootfs is a read-only, ephemeral image with no host data, so
  `/etc`, `/usr`, `/bin`, `/sbin`, `/lib`, `/opt`, `/var` etc. can be read without prompting (OS-level protections like
  `/etc/shadow`'s root-only mode still apply). Credential files (`auth.json`, `account.json`, `service.json`) are
  denied to the agent's tools outright; the session database, your git config under `/opencode/config/git`, and
  `*.env*` files keep prompting. (opencode reads its own global config directory without prompting.)
- **Agents can never modify their own setup.** Every existing global config file (`opencode.jsonc`, `opencode.json`,
  `config.json` — opencode merges all of them) is bind-mounted **read-only** into the container, so even a compromised
  agent cannot rewrite its permissions, and hard `deny` policies (`experimental.policies`) block editing the config
  directory, the recorded shell/tool-output archives, the session database and the credential files
  `auth.json`/`account.json`/`service.json` (config and state dirs) regardless of any approval. Policies cannot be
  overridden by project configs or "allow always" approvals.
- **Shell runs without prompting** (V2's allow-by-default base policy keeps the agent productive); only `git push` is
  always denied.

Edit permissions on the **host** (the global config files in `~/.config/opencode/`) — because they are mounted
read-only, in-container commands that write global config (`opencode config set --global`, `opencode mcp add --global`)
fail with a read-only filesystem error by design. `cli.json` and `service.json` stay writable (TUI settings, serve
password). Plugin management is host-side too: `opencode plugin add` writes the global config, and the plugin surfaces —
the discovery directory (`~/.config/opencode/plugins/`), `node_modules` and the package manifests — are mounted
read-only, so a writable plugin target never exists for an agent to persist code that runs on every launch.

Prefer editing from inside the container? Run with `--edit-config` (or set `edit_config = true` in a profile): it skips
the read-only mounts for that run, so `opencode config set --global`, `opencode plugin add` and direct file edits work
in-container and persist to the host config dir. Use it deliberately — in an `--edit-config` session an agent could also
write those files via shell — and note that a profile turning it on prints a warning at launch; `--edit-config=false`
forces locked mode back on even for such a profile.

### Known limitations

- Permission rules govern the agent's *tools*; a shell command still has the container user's full filesystem authority.
  Anything an agent can `sh` can read (credentials, session data, the serve password) and — except for the read-only
  mounts — write. The hardened mounts exist to prevent *self-escalation* (rewriting permissions) and silent plugin
  persistence, not to sandbox hostile code: **the container is the trust boundary**.
- Project-level plugins (`.opencode/plugins/` in a repository the agent writes) cannot be blocked without also blocking
  workspace writes. The global `plugins` control list (`-` prefixes, e.g. `"-*"`) is the only lever against them, and a
  repository can re-enable plugins it declares itself, so treat project plugin runs as "the agent wrote your repo".
- **The container can reach your host loopback** (`host.docker.internal` is mapped to the host gateway): anything bound
  to `127.0.0.1` on the host — local databases, dashboards, other `opencode serve` instances — is reachable from the
  sandbox, and the serve password in `service.json` is readable by a shell-wielding agent. Don't run host services that
  authenticate with secrets you would not hand the agent.
- **The workspace is your real repo**: an agent can write `.git/hooks/` or rewrite remotes, and those execute on the
  **host** at the next push/checkout with your full rights (including ssh-agent). The `git push` deny only blocks
  in-session pushes.
- **`-m/--mount` binds arbitrary host paths read-write** with no allowlist (`~/.ssh`, `/`, ... are accepted); mount
  deliberately, and prefer `:ro`-style care — the passwordless sandbox is not a jail for host credentials.
- **Environment values are agent-readable** (`-e`, profile `env`, `/proc/*/environ`); treat anything passed via env as
  public to the session. Credentials belong in the workspace/`auth.json` flows you control, not in env.

## Keeping images fresh with systemd

The timer runs `opencode-docker --build` once a day and rebuilds only what changed: with `[build] base_image` unset
it resolves the newest OpenCode V2 release and the base `--pull` build is a cache hit when there is no new release.
Install it for your user:

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
- New releases are picked up automatically: when `base_image` is unset, `--build` prints the resolved release
  (e.g. `Newest OpenCode V2 release: ghcr.io/anomalyco/opencode:2.0.19`) before building. If the registry is
  unreachable it falls back to the compiled-in default and warns, so the timer never hard-fails.
- After building, `--build` also prunes dangling images (`docker image prune -f`) and keeps the build cache
  capped at 10 GB (`docker builder prune --keep-storage=10G`), so daily rebuilds don't grow disk usage unbounded.

## Usage

```sh
opencode-docker                          # run in the current directory
opencode-docker -p myenv                 # use a profile
opencode-docker --feature Rust           # enable Rust toolchain for this run
opencode-docker --edit-config            # mount the config dir read-write for in-container config/plugin edits
opencode-docker -P 8080 serve            # run `serve` (headless API + web), passing remaining args to opencode
```

The TUI runs `opencode --standalone`, so each container gets a private server over stdio (no port to expose). The
`-P <port>` flag only applies to `serve`, which runs `opencode serve --hostname 0.0.0.0 --port <port>`. Note that
`serve` prints a random server password you'll need to connect a browser or `opencode --server` from the host; set
`OPENCODE_SERVER_PASSWORD` via a profile `env` to use a fixed one. The V1 `web` subcommand is gone in V2 (`serve`
serves both the API and web UI).

{
  description = "Run OpenCode in a Docker sandbox";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs =
    { self, nixpkgs }:
    let
      lib = nixpkgs.lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAll = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      mkPackage =
        pkgs:
        pkgs.rustPlatform.buildRustPackage {
          pname = "opencode-docker";
          version = (pkgs.lib.importTOML ./Cargo.toml).package.version;
          src = pkgs.lib.cleanSource ./.;
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [ pkgs.makeWrapper ];

          # The CLI shells out to `docker`; a user-service PATH is minimal, so
          # pin the client into the wrapper rather than relying on the host
          # environment.
          postInstall = ''
            wrapProgram $out/bin/opencode-docker \
              --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.docker-client ]}
          '';

          meta = {
            description = "Run OpenCode in a Docker container with the workspace mounted";
            mainProgram = "opencode-docker";
          };
        };
    in
    {
      packages = forAll (pkgs: {
        default = mkPackage pkgs;
      });

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
          ];
        };
      });

      # Optional Home Manager integration. `programs.opencode-docker.enable`
      # installs the CLI; `programs.opencode-docker.build.enable` additionally
      # installs the daily image-rebuild service and timer.
      homeManagerModules.default =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.programs.opencode-docker;

          serviceName = "opencode-docker-build";
        in
        {
          options.programs.opencode-docker = {
            enable = lib.mkEnableOption "opencode-docker";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
              defaultText = lib.literalExpression "opencode-docker.packages.<system>.default";
              description = "The opencode-docker package to install.";
            };

            build = {
              enable = lib.mkEnableOption "the periodic opencode-docker image rebuild";

              calendar = lib.mkOption {
                type = lib.types.str;
                default = "daily";
                description = "systemd OnCalendar expression for the image rebuild.";
              };
            };
          };

          config = lib.mkMerge [
            (lib.mkIf (cfg.enable || cfg.build.enable) {
              home.packages = [ cfg.package ];
            })

            (lib.mkIf cfg.build.enable {
              systemd.user.services.${serviceName} = {
                Unit = {
                  Description = "Build opencode Docker images (feature variants)";
                  After = [ "network-online.target" ];
                  Wants = [ "network-online.target" ];
                  StartLimitBurst = 3;
                  StartLimitIntervalSec = "10min";
                };

                Service = {
                  Type = "oneshot";
                  ExecStart = "${cfg.package}/bin/opencode-docker --build";
                  TimeoutStartSec = "1h";
                  Nice = 10;
                  IOSchedulingClass = "idle";
                  Restart = "on-failure";
                  RestartSec = "30s";
                };

                Install.WantedBy = [ "default.target" ];
              };

              systemd.user.timers.${serviceName} = {
                Unit.Description = "Periodically rebuild opencode Docker images";

                Timer = {
                  Unit = "${serviceName}.service";
                  OnCalendar = cfg.build.calendar;
                  Persistent = true;
                };

                Install.WantedBy = [ "timers.target" ];
              };
            })
          ];
        };
    };
}

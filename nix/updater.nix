{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.simit-updater;
  desired = (import ./update-config.nix {inherit lib;}) {
    inherit (cfg) schemaVersion workspaces forges profiles;
  };
  policy = pkgs.writeText "simit-updates.json" (builtins.toJSON desired);
  user = config.users.users.${cfg.user};
in {
  options.services.simit-updater =
    (import ./update-options.nix {inherit lib;})
    // {
      enable = lib.mkEnableOption "workstation-orchestrated qualified Simit CI updates";
      package = lib.mkOption {
        type = lib.types.package;
        default = pkgs.simit;
        description = "Updater executor. Profiles independently pin the qualified generator.";
      };
      runtimePackages = lib.mkOption {
        type = lib.types.listOf lib.types.package;
        default = [pkgs.git];
        description = ''
          Tools available to the updater, generator and forge adapters. Git is the
          only default. Add openssh for SSH transport, cargo for Rust metadata,
          nix for flake-backed configuration, and signing tools as required by
          this workstation's enrolled projects and Git configuration.
        '';
      };
      user = lib.mkOption {
        type = lib.types.str;
        default = "simit-updater";
        description = "Unprivileged execution user; its Git identity, signing and transport configuration are preserved.";
      };
      schedule = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Optional systemd OnCalendar schedule. Null enables manual runs only.";
      };
      waitSeconds = lib.mkOption {
        type = lib.types.ints.between 0 21600;
        default = 900;
      };
    };
  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.user != "root";
        message = "simit-updater must run as an unprivileged user";
      }
    ];
    users.users = lib.mkIf (cfg.user == "simit-updater") {
      simit-updater = {
        isSystemUser = true;
        group = "simit-updater";
        home = "/var/lib/simit-updater";
        createHome = true;
      };
    };
    users.groups = lib.mkIf (cfg.user == "simit-updater") {simit-updater = {};};
    environment.systemPackages = [cfg.package];
    environment.etc."simit/updates.json".source = policy;
    systemd.services = lib.mapAttrs' (name: _:
      lib.nameValuePair "simit-updater-${name}" {
        description = "Qualify and deploy Simit profile ${name}";
        wants = ["network-online.target"];
        after = ["network-online.target"];
        path = cfg.runtimePackages;
        environment.HOME = user.home;
        serviceConfig = {
          Type = "oneshot";
          User = cfg.user;
          Group = user.group;
          UMask = "0077";
          NoNewPrivileges = true;
          ProtectSystem = "strict";
          ProtectHome = "read-only";
          ReadWritePaths = map (workspace: workspace.stateDirectory) (builtins.attrValues desired.workspaces);
          ExecStart = lib.escapeShellArgs ["${cfg.package}/bin/simit" "projects" "update" "--config" "/etc/simit/updates.json" "--profile" name "--push" "--json" "--wait-seconds" (toString cfg.waitSeconds)];
          TimeoutStartSec = "infinity";
        };
      })
    desired.profiles;
    systemd.timers = lib.mkIf (cfg.schedule != null) (lib.mapAttrs' (name: _:
      lib.nameValuePair "simit-updater-${name}" {
        wantedBy = ["timers.target"];
        timerConfig = {
          OnCalendar = cfg.schedule;
          Persistent = true;
          RandomizedDelaySec = "5m";
        };
      })
    desired.profiles);
  };
}

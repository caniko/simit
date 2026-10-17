{nixpkgs}: let
  lib = nixpkgs.lib;
  mkConfig = import ../nix/update-config.nix {inherit lib;};
  example = {
    workspaces.projects = {
      root = "/work/custom-layout";
      stateDirectory = "/state/custom-updater";
      inventory.command.program = "/bin/inventory-adapter";
    };
    forges.enterprise = {
      host = "git.example.org";
      apiUrl = "https://git.example.org/api/v3";
      credentialFile = "/run/credentials/not-present-during-evaluation";
      checkAuthority = "777";
      adapter = {
        program = "/bin/simit";
        arguments = ["projects" "update-github-adapter"];
      };
      adapterSha256 = lib.concatStrings (lib.replicate 64 "b");
    };
    profiles.fleet = {
      workspace = "projects";
      generator = {
        executable = "/nix/store/fixture-simit/bin/simit";
        sourceRevision = lib.concatStrings (lib.replicate 40 "a");
        sourceCoordinate = "git.example.org/team/simit";
        forge = "enterprise";
        requiredChecks = [
          {
            name = "generator";
            authority = "777";
          }
        ];
      };
      projectOverrides."git.example.org/team/legacy" = {
        enable = false;
        reason = "Independent CI owner";
      };
    };
  };
  rendered = mkConfig example;
  workspaceWithState = stateDirectory:
    example
    // {
      workspaces.projects = example.workspaces.projects // {inherit stateDirectory;};
    };
  withGenerator = generator:
    example // {profiles.fleet = example.profiles.fleet // {generator = example.profiles.fleet.generator // generator;};};
  system = lib.nixosSystem {
    system = "x86_64-linux";
    modules = [
      ../nix/updater.nix
      ({pkgs, ...}: {
        system.stateVersion = "26.05";
        services.simit-updater =
          example
          // {
            enable = true;
            package = pkgs.hello;
            user = "operator";
            schedule = "hourly";
          };
        users.users.operator = {
          isNormalUser = true;
          group = "users";
        };
      })
    ];
  };
  service = system.config.systemd.services.simit-updater-fleet;
  customSystem = system.extendModules {
    modules = [
      ({pkgs, ...}: {
        services.simit-updater.runtimePackages = lib.mkForce [pkgs.git pkgs.cargo];
      })
    ];
  };
in
  assert rendered.schemaVersion == 1;
  assert rendered.profiles.fleet.select.includePaths == ["**"];
  assert rendered.forges.enterprise.credentialFile == "/run/credentials/not-present-during-evaluation";
  assert lib.all (state: !(builtins.tryEval (builtins.deepSeq (mkConfig (workspaceWithState state)) true)).success) ["/" "/work" "/work/" "/work//custom-layout" "/work/custom-layout/state"];
  assert !(builtins.tryEval (builtins.deepSeq (mkConfig (example
    // {
      workspaces =
        example.workspaces
        // {
          other = {
            root = "/state/custom-updater/source";
            stateDirectory = "/other-state";
            inventory.file = "/other-inventory.json";
          };
        };
    }))
  true)).success;
  assert !(builtins.tryEval (builtins.deepSeq (mkConfig (example // {profiles.fleet = example.profiles.fleet // {generator = example.profiles.fleet.generator // {executable = "/nix/store/../../tmp/generator";};};})) true)).success;
  assert !(builtins.tryEval (builtins.deepSeq (mkConfig (example // {profiles.fleet = example.profiles.fleet // {generator = example.profiles.fleet.generator // {sourceCoordinate = "other.example/team/simit";};};})) true)).success;
  assert !(builtins.tryEval (builtins.deepSeq (mkConfig (example // {unknownOption = true;})) true)).success;
  assert !(builtins.tryEval (builtins.deepSeq (mkConfig (withGenerator {executable = "/opt/simit";})) true)).success;
  assert (mkConfig (withGenerator {
    executable = "/opt/simit";
    executableSha256 = lib.concatStrings (lib.replicate 64 "a");
  })).profiles.fleet.generator.executable
  == "/opt/simit";
  assert !(builtins.tryEval (builtins.deepSeq (mkConfig (example // {profiles.fleet.workspace = "missing";})) true)).success;
  assert service.serviceConfig.User == "operator";
  assert service.serviceConfig.UMask == "0077";
  assert service.serviceConfig.ReadWritePaths == ["/state/custom-updater"];
  assert !(lib.any (rule: lib.hasInfix "/state/custom-updater" rule) system.config.systemd.tmpfiles.rules);
  assert !(builtins.tryEval (builtins.deepSeq (mkConfig (example // {forges.enterprise = example.forges.enterprise // {adapterSha256 = null;};})) true)).success;
  assert system.config.systemd.timers.simit-updater-fleet.timerConfig.OnCalendar == "hourly";
  assert lib.hasInfix "--profile" service.serviceConfig.ExecStart;
  assert lib.hasPrefix "${system.config.environment.etc."simit/updates.json".source} --profile " (lib.last (lib.splitString "--config " service.serviceConfig.ExecStart));
  assert map lib.getName system.config.services.simit-updater.runtimePackages == ["git"];
  assert map lib.getName customSystem.config.services.simit-updater.runtimePackages == ["git" "cargo"]; {
    inherit rendered;
    serviceVerified = true;
  }

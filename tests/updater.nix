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
      select.coordinates = ["git.example.org/team/demo"];
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
  withForge = forge: example // {forges.enterprise = example.forges.enterprise // forge;};
  withOtherForge = host:
    example
    // {
      forges =
        example.forges
        // {
          other =
            example.forges.enterprise
            // {
              inherit host;
              checkAuthority = "999";
            };
        };
    };
  withInventory = inventory: example // {workspaces.projects = example.workspaces.projects // {inherit inventory;};};
  withOtherState = stateDirectory:
    example
    // {
      workspaces =
        example.workspaces
        // {
          other = {
            root = "/work/other-projects";
            inherit stateDirectory;
            inventory.file = "/runtime/other-inventory.json";
          };
        };
    };
  accepts = value: (builtins.tryEval (builtins.deepSeq (mkConfig value) true)).success;
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
  multiWorkspaceSystem = system.extendModules {
    modules = [
      {
        services.simit-updater.workspaces.other = {
          root = "/work/other-projects";
          stateDirectory = "/state/other-updater";
          inventory.file = "/runtime/other-inventory.json";
        };
        services.simit-updater.profiles.other = example.profiles.fleet // {workspace = "other";};
      }
    ];
  };
in
  assert rendered.schemaVersion == 1;
  assert !(accepts (withOtherForge "git.example.org"));
  assert accepts (withOtherForge "other.example.org");
  assert lib.all (count: !(accepts (example // {profiles.fleet = example.profiles.fleet // {rollout.batchSize = count;};})) && !(accepts (example // {profiles.fleet = example.profiles.fleet // {rollout.maxConcurrent = count;};}))) [0 4294967296];
  assert accepts (example
    // {
      profiles.fleet =
        example.profiles.fleet
        // {
          rollout = {
            batchSize = 4294967295;
            maxConcurrent = 4294967295;
          };
        };
    });
  assert lib.all (pattern: !(accepts (example // {profiles.fleet = example.profiles.fleet // {select = example.profiles.fleet.select // {includePaths = [pattern];};};})) && !(accepts (example // {profiles.fleet = example.profiles.fleet // {select = example.profiles.fleet.select // {excludePaths = [pattern];};};}))) ["[" "[abc" "]" "a**" "**a" "a/**b" "***"];
  assert lib.all (pattern: accepts (example // {profiles.fleet = example.profiles.fleet // {select = example.profiles.fleet.select // {includePaths = [pattern];};};})) ["**" "retired/**" "repos/*/demo?" "literal"];
  assert !(accepts (example // {profiles.fleet = example.profiles.fleet // {rollout.canaries = ["git.example.org/team/typo"];};}));
  assert accepts (example // {profiles.fleet = example.profiles.fleet // {rollout.canaries = example.profiles.fleet.select.coordinates;};});
  assert lib.all (sourceRevision: !(accepts (withGenerator {inherit sourceRevision;}))) ["trunk" "abc123" (lib.concatStrings (lib.replicate 40 "A"))];
  assert lib.all (path:
    !(accepts (withInventory {file = path;}))
    && !(accepts (withInventory {command.program = path;}))
    && !(accepts (withForge {credentialFile = path;}))) ["relative.json" "../inventory" "/run/../credentials"];
  assert lib.all (credentialFile: !(accepts (withForge {inherit credentialFile;}))) ["/nix/store/fixture-token" "/nix//store/fixture-token"];
  assert accepts (withInventory {file = "/runtime/not-present-inventory.json";});
  assert lib.all (apiUrl: !(accepts (withForge {inherit apiUrl;}))) ["http://git.example.org" "https://token@git.example.org/api" "https://git.example.org/api?token=x" "https://git.example.org/api#fragment"];
  assert lib.all (host: !(accepts (withForge {inherit host;}))) ["" "git.example.org:443" "git.example.org/api"];
  assert !(accepts (withForge {adapterInputs."relative.py" = lib.concatStrings (lib.replicate 64 "f");}));
  assert lib.all (arguments: !(accepts (withForge {adapter = example.forges.enterprise.adapter // {inherit arguments;};}))) [["relative/script.py"] ["--file=/mutable/script.py"]];
  assert lib.all (sourceCoordinate: !(accepts (withGenerator {inherit sourceCoordinate;}))) ["git.example.org/team" "git.example.org/team/../simit" "git.example.org/team/a b"];
  assert lib.all (maintenanceRefPrefix: !(accepts (example // {profiles.fleet = example.profiles.fleet // {rollout = {inherit maintenanceRefPrefix;};};}))) ["maintenance" "bad..ref/" "bad.lock/" "bad//ref/" "bad@{ref/" "bad ref/"];
  assert lib.all (targetBranch: !(accepts (example // {profiles.fleet = example.profiles.fleet // {projectOverrides."git.example.org/team/demo" = {inherit targetBranch;};};}))) ["" "-option" "feature..ci" "feature/" "feature." ".hidden/ci" "feature.lock" "feature//ci" "feature@{ci" "feature ci"];
  assert lib.all (targetBranch: accepts (example // {profiles.fleet = example.profiles.fleet // {projectOverrides."git.example.org/team/demo" = {inherit targetBranch;};};})) ["trunk" "custom-layout/ci" "feature./ci"];
  assert lib.all (state: !(accepts (withOtherState state))) ["/state" "/state/custom-updater" "/state//custom-updater/child/"];
  assert accepts (withOtherState "/state/custom-updater-sibling");
  assert lib.all (hash:
    !(accepts (withGenerator {executableSha256 = hash;}))
    && !(accepts (withForge {adapterSha256 = hash;}))
    && !(accepts (withForge {adapterInputs."/runtime/adapter.py" = hash;}))) ["" "abc" (lib.concatStrings (lib.replicate 64 "A")) (lib.concatStrings (lib.replicate 64 "g"))];
  assert accepts (withForge {adapterInputs."/runtime/adapter.py" = lib.concatStrings (lib.replicate 64 "f");});
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
  assert lib.all (maxAgeSeconds: !(builtins.tryEval (builtins.deepSeq (mkConfig (example // {workspaces.projects = example.workspaces.projects // {inventory = example.workspaces.projects.inventory // {inherit maxAgeSeconds;};};})) true)).success) [0 86401];
  assert lib.all (maxAgeSeconds: (mkConfig (example // {workspaces.projects = example.workspaces.projects // {inventory = example.workspaces.projects.inventory // {inherit maxAgeSeconds;};};})).workspaces.projects.inventory.maxAgeSeconds == maxAgeSeconds) [1 86400];
  assert !(builtins.tryEval (builtins.deepSeq (mkConfig (example // {profiles.fleet = example.profiles.fleet // {select.coordinates = [];};})) true)).success;
  assert lib.all (timeoutSeconds: !(builtins.tryEval (builtins.deepSeq (mkConfig (example // {workspaces.projects = example.workspaces.projects // {inventory.command = example.workspaces.projects.inventory.command // {inherit timeoutSeconds;};};})) true)).success) [0 901];
  assert lib.all (timeoutSeconds: !(builtins.tryEval (builtins.deepSeq (mkConfig (example // {forges.enterprise = example.forges.enterprise // {adapter = example.forges.enterprise.adapter // {inherit timeoutSeconds;};};})) true)).success) [0 901];
  assert lib.all (timeoutSeconds: (mkConfig (example // {workspaces.projects = example.workspaces.projects // {inventory.command = example.workspaces.projects.inventory.command // {inherit timeoutSeconds;};};})).workspaces.projects.inventory.command.timeoutSeconds == timeoutSeconds) [1 900];
  assert lib.all (timeoutSeconds: (mkConfig (example // {forges.enterprise = example.forges.enterprise // {adapter = example.forges.enterprise.adapter // {inherit timeoutSeconds;};};})).forges.enterprise.adapter.timeoutSeconds == timeoutSeconds) [1 900];
  assert !(builtins.tryEval (builtins.deepSeq (mkConfig (withGenerator {executable = "/opt/simit";})) true)).success;
  assert (mkConfig (withGenerator {
    executable = "/opt/simit";
    executableSha256 = lib.concatStrings (lib.replicate 64 "a");
  })).profiles.fleet.generator.executable
  == "/opt/simit";
  assert !(builtins.tryEval (builtins.deepSeq (mkConfig (example // {profiles.fleet.workspace = "missing";})) true)).success;
  assert service.serviceConfig.User == "operator";
  assert service.serviceConfig.UMask == "0077";
  assert service.serviceConfig.PrivateTmp;
  assert lib.elem system.config.services.simit-updater.sandboxPackage service.path;
  assert service.serviceConfig.ReadWritePaths == ["/state/custom-updater"];
  assert multiWorkspaceSystem.config.systemd.services.simit-updater-fleet.serviceConfig.ReadWritePaths == ["/state/custom-updater"];
  assert multiWorkspaceSystem.config.systemd.services.simit-updater-other.serviceConfig.ReadWritePaths == ["/state/other-updater"];
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

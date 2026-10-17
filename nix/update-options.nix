{lib}: let
  inherit (lib) mkOption types;
  option = type: default: mkOption {inherit type default;};
  strings = types.listOf types.str;
  command = types.submodule {
    options = {
      program = mkOption {type = types.str;};
      arguments = option strings [];
      timeoutSeconds = option (types.ints.between 1 900) 120;
    };
  };
  requirement = types.submodule {
    options = {
      name = mkOption {type = types.str;};
      authority = mkOption {type = types.str;};
    };
  };
  requirements = types.listOf requirement;
in {
  schemaVersion = option (types.enum [1]) 1;
  workspaces = option (types.attrsOf (types.submodule {
    options = {
      root = mkOption {
        type = types.str;
        description = "Absolute runtime workspace path. Discovery does not grant deployment authority.";
      };
      stateDirectory = mkOption {type = types.str;};
      inventory = mkOption {
        type = types.submodule {
          options = {
            file = option (types.nullOr types.str) null;
            command = option (types.nullOr command) null;
            maxAgeSeconds = option (types.ints.between 1 86400) 300;
          };
        };
      };
    };
  })) {};
  forges = option (types.attrsOf (types.submodule {
    options = {
      host = mkOption {type = types.str;};
      apiUrl = mkOption {type = types.str;};
      credentialFile = option (types.nullOr types.str) null;
      adapter = mkOption {type = command;};
      adapterSha256 = option (types.nullOr types.str) null;
      adapterInputs = option (types.attrsOf types.str) {};
      checkAuthority = mkOption {type = types.str;};
    };
  })) {};
  profiles = option (types.attrsOf (types.submodule {
    options = {
      workspace = mkOption {type = types.str;};
      generator = mkOption {
        type = types.submodule {
          options = {
            executable = mkOption {type = types.str;};
            sourceRevision = mkOption {type = types.str;};
            sourceCoordinate = mkOption {type = types.str;};
            forge = mkOption {type = types.str;};
            executableSha256 = option (types.nullOr types.str) null;
            requiredChecks = mkOption {type = requirements;};
          };
        };
      };
      select = option (types.submodule {
        options = {
          kinds = option strings ["owned-repo"];
          lifecycles = option strings ["active"];
          access = option strings ["normal"];
          checkoutRoles = option strings ["canonical"];
          includePaths = option strings ["**"];
          excludePaths = option strings [];
          coordinates = mkOption {
            type = strings;
            default = [];
            description = "Exact canonical repository coordinates enrolled by operator policy. Each profile requires a non-empty list; inventory observations only narrow it.";
          };
        };
      }) {};
      scopes = option (types.listOf (types.enum ["ci"])) ["ci"];
      enrollCurrent = option types.bool false;
      requiredChecks = option requirements [];
      rollout = option (types.submodule {
        options = {
          canaries = option strings [];
          batchSize = option types.ints.positive 1;
          maxConcurrent = option types.ints.positive 1;
          stopOnFailure = option types.bool true;
          maintenanceRefPrefix = option types.str "simit/update/";
        };
      }) {};
      projectOverrides = option (types.attrsOf (types.submodule {
        options = {
          enable = option types.bool true;
          reason = option (types.nullOr types.str) null;
          targetBranch = option (types.nullOr types.str) null;
          requiredChecks = option requirements [];
        };
      })) {};
    };
  })) {};
}

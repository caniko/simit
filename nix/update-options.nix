{lib}: let
  inherit (lib) mkOption types;
  option = type: default: mkOption {inherit type default;};
  strings = types.listOf types.str;
  sha256 = types.strMatching "[0-9a-f]{64}";
  nonempty = types.addCheck types.str (value: builtins.match "[[:space:]]*" value == null);
  absolute = types.addCheck types.str (value:
    lib.hasPrefix "/" value
    && lib.all (part: part != "." && part != "..") (lib.splitString "/" value));
  normalized = path: "/" + lib.concatStringsSep "/" (lib.filter (part: part != "") (lib.splitString "/" path));
  coordinate = types.addCheck types.str (value: let
    parts = lib.splitString "/" value;
  in
    builtins.length parts
    >= 3
    && builtins.head parts == lib.toLower (builtins.head parts)
    && builtins.match ".*[[:space:]\\\\:?#@].*" value == null
    && lib.all (part: part != "" && part != "." && part != "..") parts);
  command = types.submodule {
    options = {
      program = mkOption {type = absolute;};
      arguments = option strings [];
      timeoutSeconds = option (types.ints.between 1 900) 120;
    };
  };
  requirement = types.submodule {
    options = {
      name = mkOption {type = nonempty;};
      authority = mkOption {type = nonempty;};
    };
  };
  requirements = types.listOf requirement;
  landing = types.enum ["direct" "pull-request"];
in {
  schemaVersion = option (types.enum [1]) 1;
  workspaces = option (types.attrsOf (types.submodule {
    options = {
      root = mkOption {
        type = absolute;
        description = "Absolute runtime workspace path. Discovery does not grant deployment authority.";
      };
      stateDirectory = mkOption {type = absolute;};
      inventory = mkOption {
        type = types.submodule {
          options = {
            file = option (types.nullOr absolute) null;
            command = option (types.nullOr command) null;
            maxAgeSeconds = option (types.ints.between 1 86400) 300;
          };
        };
      };
    };
  })) {};
  forges = option (types.attrsOf (types.submodule {
    options = {
      host = mkOption {type = types.addCheck (types.strMatching "[^/:[:space:]]+") (host: host == lib.toLower host);};
      apiUrl = mkOption {
        type = types.addCheck (types.strMatching "https://[^/:?#@[:space:]]+(:[0-9]{1,5})?(/[^?#[:space:]]*)?") (value: let
          port = builtins.match "https://[^/:?#@[:space:]]+:([0-9]+)(/.*)?" value;
        in
          port == null || lib.toIntBase10 (builtins.head port) <= 65535);
      };
      credentialFile = option (types.nullOr (types.addCheck absolute (path: !(normalized path == "/nix/store" || lib.hasPrefix "/nix/store/" (normalized path))))) null;
      adapter = mkOption {type = command;};
      adapterSha256 = option (types.nullOr sha256) null;
      adapterInputs = option (types.attrsOf sha256) {};
      checkAuthority = mkOption {type = nonempty;};
    };
  })) {};
  profiles = option (types.attrsOf (types.submodule {
    options = {
      workspace = mkOption {type = types.str;};
      generator = mkOption {
        type = types.submodule {
          options = {
            executable = mkOption {type = absolute;};
            sourceRevision = mkOption {type = types.strMatching "[0-9a-f]{40}";};
            sourceCoordinate = mkOption {type = coordinate;};
            forge = mkOption {type = types.str;};
            executableSha256 = option (types.nullOr sha256) null;
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
            type = types.listOf coordinate;
            default = [];
            description = "Exact canonical repository coordinates enrolled by operator policy. Each profile requires a non-empty list; inventory observations only narrow it.";
          };
        };
      }) {};
      scopes = option (types.listOf (types.enum ["ci"])) ["ci"];
      enrollCurrent = option types.bool false;
      requiredChecks = option requirements [];
      landing = mkOption {
        type = landing;
        default = "direct";
        description = "direct pushes the target branch; pull-request has the updater open and merge a pull request, parking until the forge reports it mergeable.";
      };
      rollout = option (types.submodule {
        options = {
          canaries = option (types.listOf coordinate) [];
          batchSize = option (types.ints.between 1 4294967295) 1;
          maxConcurrent = option (types.ints.between 1 4294967295) 1;
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
          landing = option (types.nullOr landing) null;
        };
      })) {};
    };
  })) {};
}

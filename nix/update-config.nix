{lib}: configuration: let
  evaluated = lib.evalModules {
    modules = [
      {options = import ./update-options.nix {inherit lib;};}
      {config = configuration;}
    ];
  };
  cfg = evaluated.config;
  forgeHosts = map (forge: forge.host) (builtins.attrValues cfg.forges);
  normalized = path: "/" + lib.concatStringsSep "/" (lib.filter (part: part != "") (lib.splitString "/" path));
  absolute = path: lib.hasPrefix "/" path && lib.all (part: part != "." && part != "..") (lib.splitString "/" path);
  contains = parent: child: parent == "/" || parent == child || lib.hasPrefix "${parent}/" child;
  requirements = values: builtins.length (lib.unique values) == builtins.length values;
  coordinate = value: let
    parts = lib.splitString "/" value;
  in
    builtins.length parts
    >= 3
    && builtins.head parts == lib.toLower (builtins.head parts)
    && builtins.match ".*[[:space:]\\\\:?#@].*" value == null
    && lib.all (part: part != "" && part != "." && part != "..") parts;
  validBranch = branch:
    branch
    != ""
    && !(lib.hasPrefix "-" branch)
    && !(lib.hasSuffix "." branch)
    && !(lib.hasInfix ".." branch)
    && !(lib.hasInfix "@{" branch)
    && builtins.match ".*[[:cntrl:][:space:]].*" branch == null
    && lib.all (char: !(lib.hasInfix char branch)) ["~" "^" ":" "?" "*" "[" "\\"]
    && lib.all (part: part != "" && !(lib.hasPrefix "." part) && !(lib.hasSuffix ".lock" part)) (lib.splitString "/" branch);
  validPrefix = prefix: lib.hasSuffix "/" prefix && validBranch "${prefix}0000000000000000";
  # ponytail: support literals, ? and component * / ** selectors. Bracket
  # classes need a glob parser shared with the runtime before Nix can admit them.
  validGlob = pattern:
    !(lib.hasPrefix "/" pattern)
    && !(lib.elem ".." (lib.splitString "/" pattern))
    && !(lib.hasInfix "[" pattern)
    && !(lib.hasInfix "]" pattern)
    && lib.all (part: !(lib.hasInfix "**" part) || part == "**") (lib.splitString "/" pattern);
  validArgument = forge: argument: let
    parts = lib.splitString "=" argument;
    value =
      if lib.hasPrefix "-" argument && builtins.length parts > 1
      then lib.concatStringsSep "=" (lib.tail parts)
      else argument;
  in
    if lib.hasPrefix "/" value
    then absolute value && (contains "/nix/store" value || builtins.hasAttr value forge.adapterInputs)
    else !(lib.hasInfix "/" value) && !(lib.hasInfix "\\" value) && value != "." && value != "..";
  validWorkspace = name: workspace:
    absolute workspace.root
    && absolute workspace.stateDirectory
    && lib.all (source:
      !(contains (normalized source.root) (normalized workspace.stateDirectory))
      && !(contains (normalized workspace.stateDirectory) (normalized source.root))) (builtins.attrValues cfg.workspaces)
    && lib.all (other:
      !(contains (normalized other.stateDirectory) (normalized workspace.stateDirectory))
      && !(contains (normalized workspace.stateDirectory) (normalized other.stateDirectory))) (builtins.attrValues (builtins.removeAttrs cfg.workspaces [name]))
    && ((workspace.inventory.file == null) != (workspace.inventory.command == null));
  validProfile = profile:
    builtins.hasAttr profile.workspace cfg.workspaces
    && builtins.hasAttr profile.generator.forge cfg.forges
    && absolute profile.generator.executable
    && (profile.generator.executableSha256 != null || lib.hasPrefix "/nix/store/" profile.generator.executable)
    && lib.hasPrefix "${cfg.forges.${profile.generator.forge}.host}/" profile.generator.sourceCoordinate
    && profile.scopes == ["ci"]
    && profile.select.coordinates != []
    && lib.all (canary: lib.elem canary profile.select.coordinates) profile.rollout.canaries
    && profile.generator.requiredChecks != []
    && requirements profile.generator.requiredChecks
    && requirements profile.requiredChecks
    && validPrefix profile.rollout.maintenanceRefPrefix
    && lib.all validGlob (profile.select.includePaths ++ profile.select.excludePaths)
    && lib.all lib.id (lib.mapAttrsToList (key: override:
      coordinate key
      && requirements override.requiredChecks
      && (override.targetBranch == null || validBranch override.targetBranch)
      && (override.enable || (override.reason != null && builtins.match "[[:space:]]*" override.reason == null))
      && (override.blocker == null || !override.enable))
    profile.projectOverrides);
  validForge = forge:
    absolute forge.adapter.program
    && (forge.adapterSha256 != null || lib.hasPrefix "/nix/store/" forge.adapter.program)
    && lib.all absolute (builtins.attrNames forge.adapterInputs)
    && lib.all (path: normalized path != normalized forge.adapter.program) (builtins.attrNames forge.adapterInputs)
    && lib.all (credential: credential == null || (normalized credential != normalized forge.adapter.program && lib.all (path: normalized path != normalized credential) (builtins.attrNames forge.adapterInputs))) [forge.credentialFile forge.ciCredentialFile]
    && (forge.ciCredentialFile == null || forge.credentialFile == null || normalized forge.ciCredentialFile != normalized forge.credentialFile)
    && lib.all (validArgument forge) forge.adapter.arguments;
in
  if
    lib.all lib.id (lib.mapAttrsToList validWorkspace cfg.workspaces)
    && builtins.length (lib.unique forgeHosts) == builtins.length forgeHosts
    && lib.all validForge (builtins.attrValues cfg.forges)
    && lib.all validProfile (builtins.attrValues cfg.profiles)
  then cfg
  else throw "simit updater: invalid workspace paths/inventory source or profile enrollment"

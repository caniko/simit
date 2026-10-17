{lib}: configuration: let
  evaluated = lib.evalModules {
    modules = [
      {options = import ./update-options.nix {inherit lib;};}
      {config = configuration;}
    ];
  };
  cfg = evaluated.config;
  validWorkspace = workspace:
    lib.hasPrefix "/" workspace.root
    && lib.hasPrefix "/" workspace.stateDirectory
    && workspace.root != workspace.stateDirectory
    && !(lib.hasPrefix "${workspace.root}/" workspace.stateDirectory)
    && ((workspace.inventory.file == null) != (workspace.inventory.command == null));
  validProfile = profile:
    builtins.hasAttr profile.workspace cfg.workspaces
    && builtins.hasAttr profile.generator.forge cfg.forges
    && profile.scopes == ["ci"]
    && profile.generator.requiredChecks != [];
in
  if
    lib.all validWorkspace (builtins.attrValues cfg.workspaces)
    && lib.all validProfile (builtins.attrValues cfg.profiles)
  then cfg
  else throw "simit updater: invalid workspace paths/inventory source or profile enrollment"

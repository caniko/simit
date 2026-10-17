{lib}: configuration: let
  evaluated = lib.evalModules {
    modules = [
      {options = import ./update-options.nix {inherit lib;};}
      {config = configuration;}
    ];
  };
  cfg = evaluated.config;
  normalized = path: "/" + lib.concatStringsSep "/" (lib.filter (part: part != "") (lib.splitString "/" path));
  absolute = path: lib.hasPrefix "/" path && lib.all (part: part != "." && part != "..") (lib.splitString "/" path);
  contains = parent: child: parent == "/" || parent == child || lib.hasPrefix "${parent}/" child;
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
    && profile.generator.requiredChecks != [];
  validForge = forge:
    absolute forge.adapter.program
    && (forge.adapterSha256 != null || lib.hasPrefix "/nix/store/" forge.adapter.program);
in
  if
    lib.all lib.id (lib.mapAttrsToList validWorkspace cfg.workspaces)
    && lib.all validForge (builtins.attrValues cfg.forges)
    && lib.all validProfile (builtins.attrValues cfg.profiles)
  then cfg
  else throw "simit updater: invalid workspace paths/inventory source or profile enrollment"

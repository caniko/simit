{
  lib,
  module ? import ./simit-module.nix {inherit lib;},
}: config: let
  evaluated = lib.evalModules {
    modules = [
      module
      {
        simit = config;
      }
    ];
  };
in
  lib.filterAttrs (_: value: value != null) evaluated.config.simit

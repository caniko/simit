{lib}: {
  options.simit = {
    homebrew = lib.mkOption {
      default = null;
      type = lib.types.nullOr (lib.types.submodule {
        freeformType = lib.types.attrsOf lib.types.anything;
        options = {
          name = lib.mkOption {
            default = null;
            type = lib.types.nullOr lib.types.str;
          };
          binaries = lib.mkOption {
            default = [];
            type = lib.types.listOf lib.types.str;
          };
          tap_url = lib.mkOption {
            type = lib.types.str;
          };
          download_repo = lib.mkOption {
            type = lib.types.str;
          };
        };
      });
    };

    chocolatey = lib.mkOption {
      default = null;
      type = lib.types.nullOr (lib.types.submodule {
        freeformType = lib.types.attrsOf lib.types.anything;
        options = {
          name = lib.mkOption {
            default = null;
            type = lib.types.nullOr lib.types.str;
          };
          download_repo = lib.mkOption {
            type = lib.types.str;
          };
        };
      });
    };

    scoop = lib.mkOption {
      default = null;
      type = lib.types.nullOr (lib.types.submodule {
        freeformType = lib.types.attrsOf lib.types.anything;
        options = {
          name = lib.mkOption {
            default = null;
            type = lib.types.nullOr lib.types.str;
          };
          bucket_url = lib.mkOption {
            type = lib.types.str;
          };
          download_repo = lib.mkOption {
            type = lib.types.str;
          };
        };
      });
    };
  };
}

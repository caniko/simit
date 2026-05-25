{lib}: {
  options.simit = {
    flake = lib.mkOption {
      default = {};
      type = lib.types.submodule {
        freeformType = lib.types.attrsOf lib.types.anything;
        options = {
          mode = lib.mkOption {
            default = "canonical";
            type = lib.types.enum ["canonical" "custom"];
          };
          scope = lib.mkOption {
            default = null;
            type = lib.types.nullOr (lib.types.enum ["hooks-only" "full"]);
          };
          toolchain_binding = lib.mkOption {
            default = "rustToolchain";
            type = lib.types.str;
          };
          crane_lib_binding = lib.mkOption {
            default = "craneLib";
            type = lib.types.str;
          };
          package_binding = lib.mkOption {
            default = "package";
            type = lib.types.str;
          };
          formatter_output = lib.mkOption {
            default = true;
            type = lib.types.bool;
          };
          formatting_check = lib.mkOption {
            default = true;
            type = lib.types.bool;
          };
          pre_commit_shell_hook = lib.mkOption {
            default = true;
            type = lib.types.bool;
          };
          expected_outputs = lib.mkOption {
            default = {};
            type = lib.types.submodule {
              freeformType = lib.types.attrsOf lib.types.anything;
              options = {
                packages = lib.mkOption {
                  default = [];
                  type = lib.types.listOf lib.types.str;
                };
                checks = lib.mkOption {
                  default = [];
                  type = lib.types.listOf lib.types.str;
                };
                top_level = lib.mkOption {
                  default = [];
                  type = lib.types.listOf lib.types.str;
                };
              };
            };
          };
        };
      };
    };

    ci = lib.mkOption {
      default = {};
      type = lib.types.submodule {
        freeformType = lib.types.attrsOf lib.types.anything;
        options = {
          extra_setup = lib.mkOption {
            default = [];
            type = lib.types.listOf lib.types.str;
          };
          extra_env = lib.mkOption {
            default = {};
            type = lib.types.attrsOf lib.types.str;
          };
          required_secrets = lib.mkOption {
            default = [];
            type = lib.types.listOf lib.types.str;
          };
        };
      };
    };

    release = lib.mkOption {
      default = {};
      type = lib.types.submodule {
        freeformType = lib.types.attrsOf lib.types.anything;
        options.signing = lib.mkOption {
          default = {};
          type = lib.types.submodule {
            freeformType = lib.types.attrsOf lib.types.anything;
            options = {
              key = lib.mkOption {
                default = null;
                type = lib.types.nullOr lib.types.str;
              };
              trust_root = lib.mkOption {
                default = "keys/maintainers.gpg";
                type = lib.types.str;
              };
              required = lib.mkOption {
                default = true;
                type = lib.types.bool;
              };
            };
          };
        };
        options.smoke = lib.mkOption {
          default = {};
          type = lib.types.submodule {
            freeformType = lib.types.attrsOf lib.types.anything;
            options.command = lib.mkOption {
              default = null;
              type = lib.types.nullOr lib.types.str;
            };
          };
        };
      };
    };

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

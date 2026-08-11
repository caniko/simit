{
  description = "Semver-aware git commit helper for Rust projects";

  inputs = {
    rs-harbor.url = "github:caniko/rs-harbor/c26b735eede8078f795651c4a9cbf0be8733b221";

    nixpkgs.follows = "rs-harbor/nixpkgs";
    rust-overlay.follows = "rs-harbor/rust-overlay";
    crane.follows = "rs-harbor/crane";
    flake-utils.url = "github:numtide/flake-utils";
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    git-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    plinth = {
      url = "git+https://codeberg.org/caniko/plinth.git";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };
  };

  outputs = {
    self,
    advisory-db,
    nixpkgs,
    rs-harbor,
    plinth,
    flake-utils,
    rust-overlay,
    treefmt-nix,
    git-hooks,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (system: let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [(import rust-overlay)];
      };

      toolchain = rs-harbor.lib.mkToolchain {
        inherit pkgs;
        toolchainProfile = "nightly";
      };
      inherit (toolchain) craneLib;
      cross = rs-harbor.lib.mkCross {inherit pkgs system;};
      simitVersion = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
      # `nix run git+https://codeberg.org/caniko/simit.git` is the public CLI
      # distribution path and must work on runners without canix's managed
      # sccache transport. Keep cached derivations for Simit's own checks,
      # but make the default runnable package self-contained.
      publicCraneLib =
        (rs-harbor.lib.mkToolchain {
          inherit pkgs;
          toolchainProfile = "nightly";
          cache.enable = false;
        }).craneLib;

      # The generator embeds the immutable action registry at compile time.
      # crane's default Cargo filter intentionally drops root JSON files, so
      # keep this one alongside the normal Cargo source set.
      src = pkgs.lib.cleanSourceWith {
        src = ./.;
        filter = path: type: let
          pathString = toString path;
        in
          (craneLib.filterCargoSources path type)
          || pkgs.lib.hasSuffix "ci-actions.json" pathString
          || pkgs.lib.hasSuffix "/.github" pathString
          || pkgs.lib.hasSuffix "/.github/workflows" pathString
          || pkgs.lib.hasInfix "/.github/workflows/" pathString;
      };

      commonArgs = {
        inherit src;
        strictDeps = true;
        cargoExtraArgs = "--all-features";
      };

      cargoArtifacts = craneLib.buildDepsOnly commonArgs;
      preCommitBin = pkgs.runCommand "pre-commit-bin" {} ''
        mkdir -p $out/bin
        ln -s ${pkgs.pre-commit}/bin/pre-commit $out/bin/pre-commit
      '';

      package = craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
          nativeBuildInputs = [preCommitBin];
          nativeCheckInputs = [pkgs.git pkgs.gnupg];
        });

      publicCargoArtifacts = publicCraneLib.buildDepsOnly commonArgs;
      publicPackage = publicCraneLib.buildPackage (commonArgs
        // {
          cargoArtifacts = publicCargoArtifacts;
          nativeBuildInputs = [preCommitBin];
          nativeCheckInputs = [pkgs.git pkgs.gnupg];
        });

      staticPackages =
        if system == "x86_64-linux"
        then
          rs-harbor.lib.mkCrossPackages {
            inherit pkgs cross;
            craneLib = publicCraneLib;
            pname = "simit";
            commonArgs =
              commonArgs
              // {
                version = simitVersion;
                doCheck = false;
              };
            targets = ["x86_64-linux-musl"];
          }
        else {};

      binaryRelease =
        if system == "x86_64-linux"
        then
          rs-harbor.lib.mkBinaryRelease {
            inherit pkgs;
            pname = "simit";
            version = simitVersion;
            artifacts.x86_64-linux-musl = {
              package = staticPackages.simit-x86_64-linux-musl;
              system = "x86_64-linux";
              rustTarget = "x86_64-unknown-linux-musl";
              binutils = pkgs.pkgsStatic.stdenv.cc.bintools;
              strip = "${pkgs.pkgsStatic.stdenv.cc.bintools}/bin/x86_64-unknown-linux-musl-strip";
              readelf = "${pkgs.pkgsStatic.stdenv.cc.bintools.bintools}/bin/x86_64-unknown-linux-musl-readelf";
              binaries = ["simit"];
            };
          }
        else null;

      treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);
      pre-commit-check = git-hooks.lib.${system}.run {
        src = ./.;
        hooks = import ./nix/pre-commit.nix {
          inherit pkgs;
          treefmtWrapper = treefmtEval.config.build.wrapper;
          rustToolchain = toolchain.rustToolchain;
        };
      };

      depsCheck = publicCargoArtifacts;

      clippyCheck = publicCraneLib.cargoClippy (commonArgs
        // {
          cargoArtifacts = publicCargoArtifacts;
          cargoClippyExtraArgs = "--all-targets -- --deny warnings";
        });

      fmtCheck = publicCraneLib.cargoFmt {
        inherit src;
      };

      nextestCheck = publicCraneLib.cargoNextest (commonArgs
        // {
          cargoArtifacts = publicCargoArtifacts;
          nativeCheckInputs = [pkgs.git pkgs.gnupg];
        });

      docCheck = publicCraneLib.cargoDoc (commonArgs
        // {
          cargoArtifacts = publicCargoArtifacts;
          cargoDocExtraArgs = "--no-deps";
        });

      auditCheck = publicCraneLib.cargoAudit {
        inherit advisory-db src;
      };

      denyCheck = publicCraneLib.cargoDeny {
        inherit src;
      };

      docs = pkgs.stdenv.mkDerivation {
        pname = "simit-docs";
        inherit (package) version;
        src = ./docs;
        nativeBuildInputs = [pkgs.mdbook];
        phases = ["buildPhase" "installPhase"];
        buildPhase = ''
          cp -r --no-preserve=mode $src docs
          mdbook build docs
        '';
        installPhase = ''
          cp -r docs/book $out
        '';
      };
      website = plinth.lib.${system}.mkProjectSite {
        pname = "simit-website";
        domain = "simit.tartanoglu.com";
        configPath = ./website/plinth-project.toml;
        docsPackage = docs;
      };
    in {
      packages =
        {
          default = publicPackage;
          docs = docs;
          website = website;
          site = website;
        }
        // (
          if binaryRelease != null
          then {
            simit-x86_64-linux-musl = staticPackages.simit-x86_64-linux-musl;
            release-bundle = binaryRelease.releaseBundle;
          }
          else {}
        );

      apps.deploy-pages = plinth.lib.${system}.mkDeployPagesApp {
        domain = "simit.tartanoglu.com";
      };

      formatter = treefmtEval.config.build.wrapper;

      checks = {
        default = publicPackage;
        formatting = treefmtEval.config.build.check self;

        # Exposes the crane deps closure so atlas's post-build hook and CI
        # Attic push can cache the project dependency graph independently.
        simit-deps = depsCheck;
        simit-clippy = clippyCheck;
        simit-fmt = fmtCheck;
        simit-nextest = nextestCheck;
        simit-doc = docCheck;
        simit-audit = auditCheck;
        simit-deny = denyCheck;

        clippy = clippyCheck;
        fmt = fmtCheck;
        nextest = nextestCheck;
        doc = docCheck;
        audit = auditCheck;
        deny = denyCheck;
        default-package-is-publicly-buildable = assert !(publicPackage.passthru.rsHarborBuildCacheWrapped or false);
          pkgs.runCommand "check-simit-default-package-cache-policy" {} "touch $out";
      };

      devShells = let
        docsPackages = with pkgs; [
          mdbook
          plinth.packages.${system}.plinth-project
          pre-commit
          rust-analyzer
        ];
      in {
        default = publicCraneLib.devShell {
          checks = self.checks.${system};
          packages = with pkgs;
            [
              alejandra
              cargo-audit
              cargo-deny
              cargo-nextest
              git
              mdbook
              prettier
              pre-commit
              rust-analyzer
              taplo
            ]
            ++ pre-commit-check.enabledPackages;
          shellHook = pre-commit-check.shellHook;
        };

        docs = publicCraneLib.devShell {
          checks = self.checks.${system};
          packages = docsPackages ++ pre-commit-check.enabledPackages;
          shellHook = pre-commit-check.shellHook;
        };
      };
    })
    // {
      lib = {
        simitModule = import ./nix/simit-module.nix {lib = nixpkgs.lib;};
        mkSimitConfig = import ./nix/simit-config.nix {lib = nixpkgs.lib;};
      };
    };
}

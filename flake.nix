{
  description = "Semver-aware git commit helper for Rust projects";

  inputs = {
    rs-harbor.url = "git+https://codeberg.org/caniko/rs-harbor.git";

    nixpkgs.follows = "rs-harbor/nixpkgs";
    rust-overlay.follows = "rs-harbor/rust-overlay";
    crane.follows = "rs-harbor/crane";
    flake-utils.follows = "rs-harbor/flake-utils";
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    git-hooks = {
      url = "github:cachix/git-hooks.nix";
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

      toolchain = rs-harbor.lib.mkToolchain {inherit pkgs;};
      inherit (toolchain) craneLib;

      src = craneLib.cleanCargoSource ./.;

      commonArgs = {
        inherit src;
        strictDeps = true;
        cargoExtraArgs = "--all-features";
      };

      cargoArtifacts = craneLib.buildDepsOnly commonArgs;

      package = craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
          nativeCheckInputs = [pkgs.git];
        });

      treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);
      pre-commit-check = git-hooks.lib.${system}.run {
        src = ./.;
        hooks = import ./nix/pre-commit.nix {
          inherit pkgs;
          treefmtWrapper = treefmtEval.config.build.wrapper;
          rustToolchain = toolchain.rustToolchain;
        };
      };

      depsCheck = cargoArtifacts;

      clippyCheck = craneLib.cargoClippy (commonArgs
        // {
          inherit cargoArtifacts;
          cargoClippyExtraArgs = "--all-targets -- --deny warnings";
        });

      fmtCheck = craneLib.cargoFmt {
        inherit src;
      };

      nextestCheck = craneLib.cargoNextest (commonArgs
        // {
          inherit cargoArtifacts;
          nativeCheckInputs = [pkgs.git];
        });

      docCheck = craneLib.cargoDoc (commonArgs
        // {
          inherit cargoArtifacts;
          cargoDocExtraArgs = "--no-deps";
        });

      auditCheck = craneLib.cargoAudit {
        inherit advisory-db src;
      };

      denyCheck = craneLib.cargoDeny {
        inherit src;
      };
    in {
      packages.default = package;

      formatter = treefmtEval.config.build.wrapper;

      checks = {
        default = package;
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
      };

      devShells.default = craneLib.devShell {
        checks = self.checks.${system};
        packages = with pkgs;
          [
            alejandra
            cargo-audit
            cargo-deny
            cargo-nextest
            git
            prettier
            pre-commit
            rust-analyzer
            taplo
          ]
          ++ pre-commit-check.enabledPackages;
        shellHook = pre-commit-check.shellHook;
      };
    });
}

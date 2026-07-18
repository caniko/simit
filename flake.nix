{
  description = "Semver-aware git commit helper for Rust projects";

  inputs = {
    rs-harbor.url = "git+https://codeberg.org/caniko/rs-harbor.git?ref=trunk&rev=9bfa8bdb0ecb22d7bc11448665f7fbaebae7a759";

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

      toolchain = rs-harbor.lib.mkToolchain {inherit pkgs;};
      inherit (toolchain) craneLib;

      # The generator embeds the immutable action registry at compile time.
      # crane's default Cargo filter intentionally drops root JSON files, so
      # keep this one alongside the normal Cargo source set.
      src = pkgs.lib.cleanSourceWith {
        src = ./.;
        filter = path: type:
          (craneLib.filterCargoSources path type)
          || pkgs.lib.hasSuffix "ci-actions.json" (toString path);
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
          nativeCheckInputs = [pkgs.git pkgs.gnupg];
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
      packages = {
        default = package;
        docs = docs;
        website = website;
        site = website;
      };

      apps.deploy-pages = plinth.lib.${system}.mkDeployPagesApp {
        domain = "simit.tartanoglu.com";
      };

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

      devShells = let
        docsPackages = with pkgs; [
          mdbook
          plinth.packages.${system}.plinth-project
          pre-commit
          rust-analyzer
        ];
      in {
        default = craneLib.devShell {
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

        docs = craneLib.devShell {
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

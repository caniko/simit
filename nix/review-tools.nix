# Self-contained tools for a controller deployment. Its direct nixpkgs input
# owns tool versions; the engine source is this exact Simit flake revision.
{
  source,
  system,
  nixpkgs,
  engineRepository ? "caniko/simit",
}: let
  pkgs = import nixpkgs {
    inherit system;
    config.allowDeprecatedx86_64Darwin = true;
  };
  python = pkgs.python3.withPackages (ps: [(ps.toPythonModule pkgs.nixpkgs-review)]);
  selector = (pkgs.writeShellScriptBin "repo-review-nixpkgs-select" ''
    exec ${python}/bin/python ${source}/src/review/assets/nixpkgs.py "$@"
  '').overrideAttrs (old: {passthru = (old.passthru or {}) // {inherit python;};});
  manifest = pkgs.writeText "simit-review-engine.json" (builtins.toJSON {
    schema_version = 1;
    repository = engineRepository;
    revision = source.rev or (throw "review-tools requires an immutable Simit revision");
    nixpkgs_revision = nixpkgs.rev;
  });
  package = assert pkgs.nixpkgs-review.version == "3.7.0";
    pkgs.rustPlatform.buildRustPackage {
      pname = "simit-review-tools";
      version = (builtins.fromTOML (builtins.readFile "${source}/Cargo.toml")).package.version;
      src = pkgs.lib.cleanSource source;
      cargoLock.lockFile = "${source}/Cargo.lock";
      nativeBuildInputs = [pkgs.makeWrapper];
      nativeCheckInputs = [pkgs.git pkgs.gnupg];
      postInstall = ''
        for binary in simit repo-review; do
          wrapProgram "$out/bin/$binary" \
            --set SIMIT_REVIEW_ENGINE_MANIFEST ${manifest} \
            --prefix PATH : ${pkgs.lib.makeBinPath [pkgs.git pkgs.nix pkgs.gh pkgs.attic-client pkgs.cachix pkgs.coreutils selector]}
        done
      '';
      meta.mainProgram = "repo-review";
    };
in {
  inherit package selector manifest;
  selectorCheck = pkgs.runCommand "simit-review-selector-check" {} ''
    ${selector}/bin/repo-review-nixpkgs-select --self-test > $out
    ${python}/bin/python ${source}/tests/fixtures/review-nixpkgs-adapter.py ${source}/src/review/assets/nixpkgs.py ${source}/tests/fixtures/review/nixpkgs/selection.json >> $out
  '';
}

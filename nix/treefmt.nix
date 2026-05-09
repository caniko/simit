{pkgs, ...}: {
  projectRootFile = "flake.nix";

  programs.rustfmt.enable = true;

  programs.alejandra.enable = true;

  programs.taplo.enable = true;

  programs.prettier = {
    enable = true;
    package = pkgs.prettier;
    includes = [
      "*.md"
      "*.markdown"
    ];
  };
}

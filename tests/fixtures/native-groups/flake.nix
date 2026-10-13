{
  description = "Native generated-group execution proof";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  outputs = {nixpkgs, ...}: {
    checks = builtins.listToAttrs (map (system: let
      pkgs = import nixpkgs {inherit system;};
      check = name:
        pkgs.runCommandNoCC "simit-native-group-${name}" {} ''
          echo 'SIMIT_NATIVE_PLATFORM_BUILDER ${system} ${name}'
          printf '%s\n' '${system} ${name}' > "$out"
        '';
    in {
      name = system;
      value = {
        runtime = check "runtime";
        sibling = check "sibling";
      };
    }) ["x86_64-linux" "aarch64-linux" "aarch64-darwin"]);
  };
}

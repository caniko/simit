{
  description = "Bounded hosted proof of Simit's build-input transport";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  outputs = {nixpkgs, ...}: let
    system = "x86_64-linux";
    pkgs = import nixpkgs {inherit system;};
    input = pkgs.runCommandNoCC "simit-transport-input" {} ''
      printf '%s\n' 'exact build input' > "$out"
    '';
    check = name:
      pkgs.runCommandNoCC "simit-transport-${name}" {} ''
        test "$(cat ${input})" = 'exact build input'
        echo 'SIMIT_FRESH_TRANSPORT_BUILDER ${name}'
        printf '%s\n' 'qualified ${name}' > "$out"
      '';
  in {
    packages.${system}.input = input;
    checks.${system} = {
      runtime = check "runtime";
      sibling = check "sibling";
    };
  };
}

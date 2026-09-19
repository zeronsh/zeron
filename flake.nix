{
  description = "Zeron: control your coding agents locally, with optional multi-device sync";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      overlays.default = final: _prev: { zeron = final.callPackage ./nix/package.nix { }; };

      packages = forAllSystems (pkgs: rec {
        zeron = pkgs.callPackage ./nix/package.nix { };
        default = zeron;
      });

      devShells = forAllSystems (
        pkgs:
        let
          zeron = self.packages.${pkgs.stdenv.hostPlatform.system}.zeron;
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [ zeron ];
            packages = with pkgs; [
              cargo
              rustc
              clippy
              rustfmt
              rust-analyzer
            ];
            env = {
              RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
              # Same dlopen'ed libraries nix/package.nix adds to the RPATH.
              LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (
                with pkgs;
                [
                  wayland
                  vulkan-loader
                  libglvnd
                ]
              );
            };
          };
        }
      );

      formatter = forAllSystems (pkgs: pkgs.nixfmt-tree);
    };
}

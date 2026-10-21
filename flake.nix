{
  description = "Blanco, a SQL editor and database client";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forEachSystem = function: nixpkgs.lib.genAttrs systems (system: function nixpkgs.legacyPackages.${system});

      # Built from the package set it is given, so the overlay links against
      # the system's own glibc, which its graphics drivers were built with.
      blancoFor = pkgs: pkgs.callPackage ./nix/package.nix { craneLib = crane.mkLib pkgs; };
    in
    {
      packages = forEachSystem (pkgs: rec {
        blanco = blancoFor pkgs;
        default = blanco;
      });

      overlays.default = final: prev: { blanco = blancoFor final; };

      devShells = forEachSystem (
        pkgs:
        let
          blanco = self.packages.${pkgs.stdenv.hostPlatform.system}.blanco;
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [ blanco ];
            packages = with pkgs; [
              cargo
              clippy
              rustc
              rustfmt
              rust-analyzer
              nodejs
            ];
            # cargo run starts Blanco from target/, which has no rpath to the
            # libraries GPUI loads at runtime.
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath blanco.runtimeLibraries;
          };
        }
      );

      formatter = forEachSystem (pkgs: pkgs.nixfmt);
    };
}

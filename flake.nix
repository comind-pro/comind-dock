{
  description = "comind-dock: terminal-native runtime and multiplexer for AI coding agents";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "cdock";
          version = cargoToml.package.version;
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
        };

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [ cargo rustc rust-analyzer clippy rustfmt ];
        };
      });
}

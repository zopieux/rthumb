{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };
        rust = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" ];
          # targets = [ "x86_64-unknown-linux-gnu" "wasm32-unknown-unknown" ];
        };
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "rthumb";
          version = "local";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
        };
        devShell = pkgs.mkShell {
          buildInputs = [
            rust
            pkgs.cargo-edit
            pkgs.extism-cli
          ];
        };
      }
    );
}

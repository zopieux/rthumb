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
        };
        rustPlatform = pkgs.makeRustPlatform {
          rustc = rust;
          cargo = rust;
        };
        nativeBuildInputs = with pkgs;[
          pkg-config
          libiconv
          llvmPackages.clang
        ];
        buildInputs = with pkgs; [
          ffmpeg
          glib
        ];
        LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
      in
      {
        packages.default = rustPlatform.buildRustPackage {
          pname = "rthumb";
          version = "local";
          src = ./.;
          inherit nativeBuildInputs buildInputs LIBCLANG_PATH;
          cargoLock.lockFile = ./Cargo.lock;
          cargoLock.outputHashes = {
            "ffmpegthumbnailer-rs-0.2.1" = "sha256-ciGTY/tEJQw8ZUfb8CDvxr2KaHvSs/JXuXL+FCC0r3s=";
          };
        };
        devShell = pkgs.mkShell {
          inherit nativeBuildInputs LIBCLANG_PATH;
          buildInputs = buildInputs ++ [
            rust
            pkgs.cargo-edit
          ];
        };
      }
    );
}

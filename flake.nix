{
  description = "TinyHarness - AI Coding Harness";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane = {
      url = "github:ipetkov/crane";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, crane, rust-overlay, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        inherit (pkgs) lib;

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rustfmt" "clippy" ];
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        src = lib.cleanSourceWith {
          filter = path: type:
            (craneLib.filterCargoSources path type) ||
            (lib.hasSuffix ".md" path);
          src = lib.cleanSource ./.;
        };

        commonArgs = {
          inherit src;
          strictDeps = true;
          buildInputs = [
            pkgs.openssl
          ] ++ lib.optionals pkgs.stdenv.isDarwin [
            pkgs.apple-sdk
          ];
          nativeBuildInputs = [
            pkgs.pkg-config
          ];
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        tinyharness = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          doCheck = false;
          meta = {
            mainProgram = "tinyharness";
          };
        });

        tests = craneLib.cargoTest (commonArgs // {
          inherit cargoArtifacts;
        });

      in {
        packages = {
          default = tinyharness;
          tinyharness = tinyharness;
        };

        checks = {
          inherit tinyharness;
          inherit tests;
          clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "-- --deny warnings";
          });
          fmt = craneLib.cargoFmt {
            inherit src;
          };
        };

        devShells.default = craneLib.devShell {
          checks = self.checks.${system};
          packages = with pkgs; [
            cargo
            clippy
            rust-analyzer
            rustfmt
          ];
        };
      });
}

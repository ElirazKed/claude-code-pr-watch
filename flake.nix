{
  description = "prw: a PR manager for the terminal (companion to the pr-watch Claude Code mod)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      # No x86_64-darwin: nixpkgs-unstable (26.11) dropped it, and evaluating
      # anything for it there is an error. `cargo install` still works on Intel Macs.
      systems = [
        "aarch64-darwin"
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      cargoToml = lib.importTOML ./companion/Cargo.toml;
    in
    {
      packages = forAllSystems (pkgs: {
        prw = pkgs.rustPlatform.buildRustPackage {
          pname = "prw";
          inherit (cargoToml.workspace.package) version;
          # Only the Rust sources, and the status fixtures and demo cache their
          # tests read (include_str!): a change to the mod's code, the docs or the
          # README leaves the store path (and so the build) as it was.
          src = lib.fileset.toSource {
            root = ./companion;
            fileset = lib.fileset.unions [
              ./companion/Cargo.toml
              ./companion/Cargo.lock
              ./companion/crates
              ./companion/fixtures
              ./companion/demo
            ];
          };
          cargoLock.lockFile = ./companion/Cargo.lock;
          # checkPhase runs `cargo test` over the whole workspace.
          meta = {
            description = "All your PRs in one terminal: a PR manager (companion to the pr-watch mod)";
            homepage = "https://github.com/ElirazKed/claude-code-pr-watch";
            license = lib.licenses.mit;
            mainProgram = "prw";
            platforms = systems;
          };
        };
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.prw;
      });

      checks = forAllSystems (pkgs: {
        prw = self.packages.${pkgs.stdenv.hostPlatform.system}.prw;
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
          ];
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}

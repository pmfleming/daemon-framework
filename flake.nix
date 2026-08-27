{
  description = "Shared Rust infrastructure and process services for Shelllist daemons";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          shelllistSearch = pkgs.rustPlatform.buildRustPackage {
            pname = "shelllist-search";
            version = "0.1.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [ "-p" "shelllist-search" ];
            cargoTestFlags = [ "-p" "shelllist-search" ];
            meta = {
              description = "Typo-tolerant fuzzy result ranking for Shelllist";
              homepage = "https://github.com/pmfleming/daemon-framework";
              license = pkgs.lib.licenses.mit;
              mainProgram = "shelllist-search";
              platforms = pkgs.lib.platforms.linux;
            };
          };
        in
        {
          inherit shelllistSearch;
          default = shelllistSearch;
        });

      checks = forAllSystems (system: {
        shelllistSearch = self.packages.${system}.shelllistSearch;
      });

      apps = forAllSystems (system: {
        shelllistSearch = {
          type = "app";
          program = "${self.packages.${system}.shelllistSearch}/bin/shelllist-search";
        };
        default = self.apps.${system}.shelllistSearch;
      });

      formatter = forAllSystems (system:
        (import nixpkgs { inherit system; }).nixpkgs-fmt);

      devShells = forAllSystems (system:
        let pkgs = import nixpkgs { inherit system; };
        in {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              cargo-audit
              clippy
              nixpkgs-fmt
              rust-analyzer
              rustc
              rustfmt
            ];
          };
        });
    };
}

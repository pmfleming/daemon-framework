{
  description = "Shared Rust infrastructure for Shelllist daemons";

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
          protocolBindings = pkgs.rustPlatform.buildRustPackage {
            pname = "shelllist-protocol-js";
            version = "0.1.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [ "-p" "shelllist-daemon-core" "--bin" "shelllist-protocol-js" ];
            cargoTestFlags = [ "-p" "shelllist-daemon-core" "--bin" "shelllist-protocol-js" ];
            meta = {
              description = "Generate Shelllist JavaScript bindings from daemon protocol registries";
              homepage = "https://github.com/pmfleming/daemon-framework";
              license = pkgs.lib.licenses.mit;
              mainProgram = "shelllist-protocol-js";
              platforms = pkgs.lib.platforms.linux;
            };
          };
        in
        {
          inherit protocolBindings;
          default = protocolBindings;
        });

      checks = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
        in {
          protocolBindings = self.packages.${system}.protocolBindings;
          workspace = pkgs.rustPlatform.buildRustPackage {
            pname = "daemon-framework-workspace-check";
            version = "0.1.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [ "--workspace" ];
            cargoTestFlags = [ "--workspace" ];
            installPhase = "touch $out";
          };
        });

      apps = forAllSystems (system: {
        protocolBindings = {
          type = "app";
          program = "${self.packages.${system}.protocolBindings}/bin/shelllist-protocol-js";
        };
        default = self.apps.${system}.protocolBindings;
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

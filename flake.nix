{
  description = "Shared Rust infrastructure for Shelllist daemons";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
  inputs.crane.url = "github:ipetkov/crane/47b6b27ed9a3a9181415e4367d0c30ab2a0e0250";

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
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      lib = {
        # The tools and workspace check share one compiled dependency layer.
        workspaceArtifacts =
          pkgs:
          (self.lib.buildRustPackage pkgs {
            pname = "daemon-framework-workspace-check";
            version = "0.1.0";
            src = self.lib.workspaceSource pkgs;
            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [ "--workspace" ];
            cargoTestFlags = [ "--workspace" ];
          }).cargoArtifacts;
        buildRustPackage =
          pkgs:
          import ./nix/build-rust-package.nix {
            inherit pkgs;
            craneLib = crane.mkLib pkgs;
          };
        # Exclude deployment tooling and the unrelated local-build crate from
        # daemon path dependencies, while preserving shared workspace metadata.
        daemonSource =
          pkgs:
          let
            manifest = builtins.fromTOML (builtins.readFile ./Cargo.toml);
            members = [
              "shelllist-daemon-core"
              "shelllist-daemon-tokio"
              "shelllist-hyprland"
            ];
            workspace = (pkgs.formats.toml { }).generate "Cargo.toml" (
              manifest
              // {
                workspace = manifest.workspace // {
                  members = map (name: "crates/${name}") members;
                };
              }
            );
            source = pkgs.lib.fileset.toSource {
              root = ./.;
              fileset = pkgs.lib.fileset.unions (map (name: ./crates + "/${name}") members);
            };
          in
          pkgs.runCommand "daemon-framework-source" { } ''
            mkdir -p $out
            cp -R ${source}/crates $out/crates
            cp ${workspace} $out/Cargo.toml
          '';
        workspaceSource =
          pkgs:
          pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./crates
            ];
          };
      };

      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          protocolBindings = self.lib.buildRustPackage pkgs {
            pname = "shelllist-protocol-js";
            version = "0.1.0";
            cargoArtifacts = self.lib.workspaceArtifacts pkgs;
            src = self.lib.workspaceSource pkgs;
            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [
              "-p"
              "shelllist-daemon-core"
              "--bin"
              "shelllist-protocol-js"
            ];
            cargoTestFlags = [
              "-p"
              "shelllist-daemon-core"
              "--bin"
              "shelllist-protocol-js"
            ];
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
          localBuild = self.lib.buildRustPackage pkgs {
            pname = "local-build";
            version = "0.1.0";
            cargoArtifacts = self.lib.workspaceArtifacts pkgs;
            src = self.lib.workspaceSource pkgs;
            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [
              "-p"
              "shelllist-local-build"
            ];
            cargoTestFlags = [
              "-p"
              "shelllist-local-build"
            ];
            nativeBuildInputs = [ pkgs.makeWrapper ];
            nativeCheckInputs = [ pkgs.git ];
            postInstall = ''
              wrapProgram $out/bin/local-build \
                --prefix PATH : ${
                  pkgs.lib.makeBinPath [
                    pkgs.git
                    pkgs.nix
                  ]
                }
            '';
            # Fixtures intercept Nix through PATH, without the runtime wrapper.
            passthru.unwrappedProgram = "${self.packages.${system}.localBuild}/bin/.local-build-wrapped";
            meta = {
              description = "Build one snapshot of the current local Shelllist development graph";
              license = pkgs.lib.licenses.mit;
              mainProgram = "local-build";
              platforms = pkgs.lib.platforms.linux;
            };
          };
          default = protocolBindings;
        }
      );

      checks = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          localBuild = self.packages.${system}.localBuild;
          protocolBindings = self.packages.${system}.protocolBindings;
          workspace = self.lib.buildRustPackage pkgs {
            pname = "daemon-framework-workspace-check";
            version = "0.1.0";
            cargoArtifacts = self.lib.workspaceArtifacts pkgs;
            src = self.lib.workspaceSource pkgs;
            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [ "--workspace" ];
            cargoTestFlags = [ "--workspace" ];
            nativeCheckInputs = [
              pkgs.dbus
              pkgs.git
            ];
            installPhase = "touch $out";
          };
        }
      );

      apps = forAllSystems (system: {
        localBuild = {
          type = "app";
          program = "${self.packages.${system}.localBuild}/bin/local-build";
        };
        protocolBindings = {
          type = "app";
          program = "${self.packages.${system}.protocolBindings}/bin/shelllist-protocol-js";
        };
        default = self.apps.${system}.protocolBindings;
      });

      formatter = forAllSystems (system: (import nixpkgs { inherit system; }).nixpkgs-fmt);

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              cargo-audit
              dbus
              git
              clippy
              nixpkgs-fmt
              rust-analyzer
              rustc
              rustfmt
            ];
          };
        }
      );
    };
}

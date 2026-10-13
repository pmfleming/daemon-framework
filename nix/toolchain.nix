# Nix does not read rustup's channel file. Fail closed rather than silently
# compiling with a different compiler after a nixpkgs/host override.
pkgs:
let
  expected = (builtins.fromTOML (builtins.readFile ../rust-toolchain.toml)).toolchain.channel;
  actual = {
    rustc = pkgs.rustc.version;
    cargo = pkgs.cargo.version;
    clippy = pkgs.clippy.version;
    rustfmt = pkgs.rustfmt.version;
    platformRustc = pkgs.rustPlatform.rust.rustc.version;
    platformCargo = pkgs.rustPlatform.rust.cargo.version;
  };
in
assert pkgs.lib.assertMsg (
  pkgs.stdenv.hostPlatform.system == "x86_64-linux"
) "Shelllist supports x86_64-linux (primarily modern Intel/AMD PCs) only";
assert pkgs.lib.assertMsg (builtins.all (version: version == expected) (builtins.attrValues actual))
  "Shelllist requires Rust/Cargo ${expected} in builds and developer shells; update the product build-input policy deliberately";
{
  inherit expected actual;
  inherit (pkgs) rustPlatform;
  packages = [
    pkgs.rustc
    pkgs.cargo
    pkgs.clippy
    pkgs.rustfmt
  ];
  check =
    pkgs.runCommand "shelllist-toolchain-${expected}"
      {
        nativeBuildInputs = [
          pkgs.rustc
          pkgs.cargo
          pkgs.clippy
          pkgs.rustfmt
        ];
      }
      ''
        mkdir -p $out
        rustc -Vv | tee $out/rustc.txt
        cargo -Vv | tee $out/cargo.txt
        rustfmt --version | tee $out/rustfmt.txt
        cargo-clippy --version | tee $out/clippy.txt
        test "$(rustc --version | cut -d ' ' -f 2)" = '${expected}'
        test "$(cargo --version | cut -d ' ' -f 2)" = '${expected}'
        printf '%s\n' '${builtins.toJSON actual}' > $out/versions.json
      '';
}

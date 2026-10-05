# Small adapter for our existing buildRustPackage call sites. Runtime/test hooks
# belong only to the real package; the dependency layer contains dummy targets.
{ pkgs, craneLib }:
args:
let
  inherit (pkgs) lib;
  cargoExtraArgs = "--locked --offline";
  cargoBuildExtraArgs = lib.escapeShellArgs (args.cargoBuildFlags or [ ]);
  cargoTestExtraArgs = lib.escapeShellArgs (
    (args.cargoTestFlags or [ ]) ++ [ "--" ] ++ (args.checkFlags or [ ])
  );
  cargoVendorDir = craneLib.vendorCargoDeps {
    cargoLock = args.cargoLock.lockFile;
    # PipeWire's bindgen macro fallback otherwise writes beside read-only
    # vendored sources and silently drops SPA_ID_INVALID / PW_ID_ANY. Keep
    # generated files in Cargo's writable OUT_DIR (both bar and bt use these).
    overrideVendorCargoPackage =
      package: drv:
      if
        builtins.elem package.name [
          "libspa-sys"
          "pipewire-sys"
        ]
        && package.version == "0.10.0"
      then
        drv.overrideAttrs (old: {
          postPatch = (old.postPatch or "") + ''
            substituteInPlace build.rs --replace-fail \
              '.clang_macro_fallback()' \
              '.clang_macro_fallback().clang_macro_fallback_build_dir(std::env::var("OUT_DIR").unwrap())'
          '';
        })
      else
        drv;
  };
  common =
    builtins.removeAttrs args [
      "cargoLock"
      "cargoBuildFlags"
      "cargoTestFlags"
      "checkFlags"
      "passthru"
    ]
    // {
      inherit
        cargoExtraArgs
        cargoBuildExtraArgs
        cargoTestExtraArgs
        cargoVendorDir
        ;
      strictDeps = true;
    };
  # Keep nativeCheckInputs in both layers: differing PKG_CONFIG_PATH values
  # can otherwise force native dependencies to recompile in the real package.
  cargoArtifacts =
    args.cargoArtifacts or (craneLib.buildDepsOnly (
      builtins.removeAttrs common [
        "postInstall"
        "postFixup"
        "installPhase"
        "HYPRIDLE_TEST_BIN"
        "meta"
      ]
      // {
        # Compile test dependencies, but never execute dummy tests or real hooks.
        cargoTestExtraArgs = lib.escapeShellArgs ((args.cargoTestFlags or [ ]) ++ [ "--no-run" ]);
      }
    ));
in
craneLib.buildPackage (
  common
  // {
    inherit cargoArtifacts;
    passthru = (args.passthru or { }) // {
      # Explicitly retained by the deployment check cache, not the runtime closure.
      rebuildCache = [
        cargoArtifacts
        cargoVendorDir
      ];
    };
  }
)

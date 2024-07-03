{
  inputs = {
    devshell = {
      url = "github:numtide/devshell";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-utils.url = "github:numtide/flake-utils";
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    nixpkgs-esp-dev = {
      url = "github:Lindboard/nixpkgs-esp-dev";
      # FIXME: ^--- has 5.2.2, original --> "mirrexagon/nixpkgs-esp-dev";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-utils.follows = "flake-utils";
    };

    pre-commit-hooks = {
      url = "github:cachix/pre-commit-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    devshell,
    flake-utils,
    nixpkgs,
    pre-commit-hooks,
    rust-overlay,
    ...
  } @ inputs:
    flake-utils.lib.eachDefaultSystem (localSystem: let
      pkgs = import nixpkgs {
        inherit localSystem;
        overlays = [
          devshell.overlays.default
          rust-overlay.overlays.default
          inputs.nixpkgs-esp-dev.overlays.default
        ];
      };
      inherit (pkgs) lib;

      projectName = "esp-idf-matter";
      rustToolchain = pkgs.pkgsBuildHost.rust-bin.nightly.latest.default.override {
        extensions = ["rust-src"];
        targets = ["riscv32imac-unknown-none-elf"];
      };
    in {
      checks.pre-commit = pre-commit-hooks.lib.${localSystem}.run {
        src = ./.;
        hooks = {
          alejandra.enable = true;
          cargo-check.enable = true;
          rustfmt.enable = true;
          statix.enable = true;
        };
      };

      # `nix develop`
      devShells.default = pkgs.devshell.mkShell {
        name = projectName;

        commands = [
          {
            package = pkgs.alejandra;
            help = "Format nix code";
          }
          {
            package = pkgs.statix;
            help = "Lint nix code";
          }
          {
            package = pkgs.deadnix;
            help = "Find unused expressions in nix code";
          }
        ];

        devshell.startup.pre-commit.text = self.checks.${localSystem}.pre-commit.shellHook;
        packages = let
          # Do not expose rust's gcc: https://github.com/oxalica/rust-overlay/issues/70
          # Create a wrapper that only exposes $pkg/bin. This prevents pulling in
          # development deps, like python interpreter + $PYTHONPATH, when adding
          # packages to a nix-shell. This is especially important when packages
          # are combined from different nixpkgs versions.
          mkBinOnlyWrapper = pkg:
            pkgs.runCommand "${pkg.pname}-${pkg.version}-bin" {inherit (pkg) meta;} ''
              mkdir -p "$out/bin"
              for bin in "${lib.getBin pkg}/bin/"*; do
                  ln -s "$bin" "$out/bin/"
              done
            '';
        in [
          (mkBinOnlyWrapper rustToolchain)
          pkgs.rust-analyzer

          pkgs.stdenv.cc
          pkgs.pkg-config
          pkgs.gnumake
          pkgs.cmake
          pkgs.ninja

          pkgs.espflash
          pkgs.python3
          pkgs.python3Packages.pip
          pkgs.python3Packages.virtualenv
          pkgs.ldproxy
        ];

        env = [
          {
            name = "IDF_PATH";
            value = pkgs.esp-idf-full.overrideAttrs (_old: {
              postFixup = ''
                # make esp-idf cmake git version detection happy
                cd $out
                git init .
                git config user.email "nixbld@localhost"
                git config user.name "nixbld"
                git commit --date="1970-01-01 00:00:00" --allow-empty -m "make idf happy"
              '';
            });
          }
          {
            name = "MCU";
            value = "esp32c6";
          }
          {
            name = "CARGO_BUILD_TARGET";
            value = "riscv32imac-esp-espidf";
          }
          {
            # Also on x86_64 a strange --target=riscv32imac_zicsr_zifencei-esp-espidf is added by default
            # https://github.com/esp-rs/esp-idf-sys/issues/223
            name = "CRATE_CC_NO_DEFAULTS";
            value = "1";
          }
          {
            name = "LD_LIBRARY_PATH";
            value = pkgs.lib.makeLibraryPath [
              pkgs.stdenv.cc.cc.lib
            ];
          }
          {
            name = "LIBCLANG_PATH";
            value = "${pkgs.llvmPackages.libclang.lib}/lib";
          }
        ];
      };

      formatter = pkgs.alejandra; # `nix fmt`
    });
}

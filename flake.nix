{
  inputs = {
    devshell = {
      url = "github:numtide/devshell";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-parts.url = "github:hercules-ci/flake-parts";
    nci = {
      url = "github:yusdacra/nix-cargo-integration";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    nixpkgs-esp-dev = {
      url = "github:mirrexagon/nixpkgs-esp-dev";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    pre-commit-hooks = {
      url = "github:cachix/pre-commit-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs:
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      imports = [
        inputs.devshell.flakeModule
        inputs.nci.flakeModule
        inputs.pre-commit-hooks.flakeModule
        inputs.treefmt-nix.flakeModule
      ];

      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];

      perSystem =
        {
          config,
          pkgs,
          system,
          ...
        }:
        let
          projectName = "esp-idf-matter";
        in
        {
          _module.args.pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = [
              inputs.nixpkgs-esp-dev.overlays.default
            ];
          };

          devshells.default = {
            packages = [
              config.treefmt.build.wrapper
              pkgs.flip-link
              pkgs.cargo-edit
              pkgs.probe-rs
              pkgs.rust-analyzer
              pkgs.espflash

              pkgs.stdenv.cc
              pkgs.pkg-config
              pkgs.gnumake
              pkgs.cmake
              pkgs.ninja

              pkgs.python3
              pkgs.python3Packages.pip
              pkgs.python3Packages.virtualenv
              pkgs.ldproxy
            ];

            env = [
              {
                name = "IDF_PATH";
                value =
                  (pkgs.esp-idf-full.override {
                    rev = "v5.3.2";
                    sha256 = "sha256-sQYylDGl7tDQzLOee3yw+Ev+oJzCyJQ7cNDXWaDkUTk=";
                  }).overrideAttrs
                    (_old: {
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
                  pkgs.libz
                  pkgs.libxml2
                ];
              }
              {
                name = "LIBCLANG_PATH";
                value = "${pkgs.llvmPackages.libclang.lib}/lib";
              }
            ];

            devshell.startup.pre-commit.text = config.pre-commit.installationScript;
          };

          pre-commit.settings.hooks.treefmt.enable = true;
          treefmt = {
            projectRootFile = "flake.nix";
            programs = {
              deadnix.enable = true;
              statix.enable = true;
              nixfmt.enable = true;
              rustfmt.enable = true;
            };
          };

          nci.projects.${projectName} = {
            path = ./.;
            numtideDevshell = "default";
          };
          nci.crates.${projectName} = { };

          packages.default = config.nci.outputs.${projectName}.packages.release;
        };
    };
}

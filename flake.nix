{
  description = "Ene assistant development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
    agent-skills.url = "github:Kyure-A/agent-skills-nix";
    wshobson-agents = {
      url = "github:wshobson/agents";
      flake = false;
    };
    apollo-skills = {
      url = "github:apollographql/skills";
      flake = false;
    };
    rust-skills = {
      url = "github:actionbook/rust-skills";
      flake = false;
    };
    everything-claude-code = {
      url = "github:affaan-m/everything-claude-code";
      flake = false;
    };
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      flake-utils,
      agent-skills,
      wshobson-agents,
      apollo-skills,
      rust-skills,
      everything-claude-code,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        agentLib = agent-skills.lib.agent-skills;

        sources = {
          wshobson = {
            path = wshobson-agents;
            subdir = "plugins/systems-programming/skills";
          };
          apollo = {
            path = apollo-skills;
            subdir = "skills";
          };
          rust-skills-src = {
            path = rust-skills;
            subdir = "skills";
          };
          everything = {
            path = everything-claude-code;
            subdir = "skills";
          };
        };

        catalog = agentLib.discoverCatalog sources;
        allowlist = agentLib.allowlistFor {
          inherit catalog sources;
          enable = [
            "rust-async-patterns"
            "rust-best-practices"
            "m15-anti-pattern"
            "rust-testing"
          ];
        };
        selection = agentLib.selectSkills {
          inherit catalog allowlist sources;
          skills = { };
        };
        bundle = agentLib.mkBundle { inherit pkgs selection; };

        localTargets = {
          opencode = agentLib.defaultLocalTargets.opencode // {
            enable = true;
          };
        };

        # Create compatibility symlinks for crates expecting standard libappindicator3.
        # This replaces running dynamic shell commands on startup (mktemp / ln -sf).
        appindicatorCompat = pkgs.runCommand "appindicator-compat" { } ''
          mkdir -p $out/lib
          ln -sfn ${pkgs.libayatana-appindicator}/lib/libayatana-appindicator3.so.1 $out/lib/libappindicator3.so.1
          ln -sfn ${pkgs.libayatana-appindicator}/lib/libayatana-appindicator3.so.1 $out/lib/libappindicator3.so
        '';
      in
      {
        devShells.default =
          with pkgs;
          mkShell {
            buildInputs = [
              # Stable Rust compiler and standard library sources
              (rust-bin.stable.latest.default.override {
                extensions = [ "rust-src" ];
                targets = [ "x86_64-pc-windows-gnu" ];
              })
              pkg-config
              rustPlatform.bindgenHook
              openssl
            ]
            ++ lib.optionals (lib.strings.hasInfix "linux" system) [
              mold
              clang
              pkgs.pkgsCross.mingwW64.stdenv.cc
              alsa-lib
              libayatana-appindicator
              mesa
              vulkan-loader
              vulkan-tools
              libudev-zero
              libgbm
              libx11
              libxcursor
              libXi
              libxrandr
              libclang
              pipewire
              wayland
              wayland-protocols
              glib
              pango
              cairo
              gdk-pixbuf
              gtk3
              libxkbcommon
              xdotool
              chromium
            ];

            # Windows cross-compilation target variables
            CC_x86_64_pc_windows_gnu = "${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}cc";
            CXX_x86_64_pc_windows_gnu = "${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}c++";
            AR_x86_64_pc_windows_gnu = "${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}ar";
            CFLAGS_x86_64-pc-windows-gnu = "-idirafter ${pkgs.pkgsCross.mingwW64.windows.pthreads}/include";
            CFLAGS_x86_64_pc_windows_gnu = "-idirafter ${pkgs.pkgsCross.mingwW64.windows.pthreads}/include";
            CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = "-L${pkgs.pkgsCross.mingwW64.windows.pthreads}/lib";

            RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
            LD_LIBRARY_PATH = lib.makeLibraryPath (
              [
                mesa
                libgbm
                vulkan-loader
                libx11
                libXi
                libxcursor
                libxkbcommon
                xdotool
              ]
              ++ lib.optionals (lib.strings.hasInfix "linux" system) [
                libayatana-appindicator
                appindicatorCompat
              ]
            );
            LIBCLANG_PATH = "${llvmPackages.libclang.lib}/lib";

            # OpenSSL locations (required by native-tls build script)
            OPENSSL_DIR = "${pkgs.openssl.dev}";
            OPENSSL_LIB_DIR = "${pkgs.openssl.out}/lib";
            OPENSSL_INCLUDE_DIR = "${pkgs.openssl.dev}/include";

            # libxdo (required by enigo)
            NIX_LDFLAGS = "-L${pkgs.xdotool}/lib";

            shellHook =
              let
                skillsHook = agentLib.mkShellHook {
                  inherit pkgs bundle;
                  targets = localTargets;
                };
              in
              ''
                export CARGO_TARGET_DIR="$PWD/target"
                if [[ "$-" == *i* ]]; then
                  set +e
                  ${skillsHook}
                  set -e
                fi
              '';
          };
      }
    );
}

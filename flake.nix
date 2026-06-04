{
  description = "bevy flake";

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
          opencode = agentLib.defaultLocalTargets.opencode // { enable = true; };
        };
      in
      {
        devShells.default =
          with pkgs;
          mkShell {
            buildInputs = [
              # Rust dependencies
              (rust-bin.nightly.latest.default.override {
                extensions = [
                  "rust-src"
                  "rustc-codegen-cranelift-preview"
                ];
              })
              pkg-config
              rustPlatform.bindgenHook
              # OpenSSL (required for native-tls)
              openssl
              diesel-cli
            ]
            ++ lib.optionals (lib.strings.hasInfix "linux" system) [
              # for Linux
              # Faster linker
              mold
              clang
              # Audio (Linux only)
              alsa-lib
              # Tray indicator compatibility library
              libayatana-appindicator
              # OS / graphics stuff
              mesa
              vulkan-loader
              # For debugging around vulkan
              vulkan-tools
              # Other dependencies
              libudev-zero
              libgbm
              libx11
              libxcursor
              libXi
              libxrandr
              libclang
              pipewire
              # Wayland (Linux only)
              wayland
              wayland-protocols
              # GLib (for glib-sys / gtk-related crates)
              glib
              # Pango/Cairo for text rendering (pango-sys, cairo-sys, pango dependencies)
              pango
              cairo
              gdk-pixbuf
              # GTK3 (provides gdk-3.0, atk, etc.)
              gtk3
              libxkbcommon
              # xdotool / libxdo for enigo (GUI automation)
              xdotool
              # Chromium for browser automation (Phase 3)
              chromium
            ];
            RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
            LD_LIBRARY_PATH = lib.makeLibraryPath [
              libayatana-appindicator
              mesa
              libgbm
              vulkan-loader
              libx11
              libXi
              libxcursor
              libxkbcommon
              xdotool
            ];
            LIBCLANG_PATH = "${llvmPackages.libclang.lib}/lib";
            # OpenSSL environment (for native-tls)
            OPENSSL_DIR = "${pkgs.openssl.dev}";
            OPENSSL_LIB_DIR = "${pkgs.openssl.out}/lib";
            OPENSSL_INCLUDE_DIR = "${pkgs.openssl.dev}/include";
            PKG_CONFIG_PATH = lib.concatStringsSep ":" [
              "${pkgs.openssl.dev}/lib/pkgconfig"
              "${pkgs.xdotool}/lib/pkgconfig"
            ];
            # libxdo (enigo dependency)
            NIX_LDFLAGS = "-L${pkgs.xdotool}/lib";
            shellHook = let
              skillsHook = agentLib.mkShellHook {
                inherit pkgs bundle;
                targets = localTargets;
              };
              appindicatorHook = lib.optionalString (lib.strings.hasInfix "linux" system) ''
                appindicator_compat_dir="$(mktemp -d -t ene-appindicator-compat-XXXXXX)"
                ln -sfn ${libayatana-appindicator}/lib/libayatana-appindicator3.so.1 "$appindicator_compat_dir/libappindicator3.so.1"
                ln -sfn ${libayatana-appindicator}/lib/libayatana-appindicator3.so.1 "$appindicator_compat_dir/libappindicator3.so"
                export LD_LIBRARY_PATH="$appindicator_compat_dir:''${LD_LIBRARY_PATH:-}"
              '';
            in ''
              ${skillsHook}
              ${appindicatorHook}
            '';
          };
      }
    );
}

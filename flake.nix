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
        # MinGW compat header for aws-lc-sys (jitterentropy needs <sched.h>)
        mingwCompatHeaders = pkgs.runCommand "mingw-compat-headers" { } ''
                    mkdir -p $out/include
                    cat > $out/include/sched.h << 'INNEREOF'
          #ifndef _SCHED_H
          #define _SCHED_H
          void __attribute__((stdcall)) Sleep(unsigned long dwMilliseconds);
          static inline int sched_yield(void) { Sleep(0); return 0; }
          #endif
          INNEREOF
        '';
        # MinGW winpthreads: Rust's x86_64-pc-windows-gnu target requires libpthread.a
        mingwPthreads = pkgs.pkgsCross.mingwW64.stdenv.mkDerivation {
          pname = "mingw-pthreads";
          version = "13.0.0";
          src = pkgs.fetchurl {
            url = "https://sourceforge.net/projects/mingw-w64/files/mingw-w64/mingw-w64-release/mingw-w64-v13.0.0.tar.bz2";
            hash = "sha256-Wv6CKvXE7b9n2q9F7sYdU49J7vaxlSTeZIl8a5WCjK8=";
          };
          nativeBuildInputs = [ pkgs.autoreconfHook ];
          configurePhase = ''
            cd mingw-w64-libraries/winpthreads
            autoreconf -fi
            ./configure --host=x86_64-w64-mingw32 --enable-static --prefix=$out
          '';
          buildPhase = ''
            make -j$NIX_BUILD_CORES
          '';
          installPhase = ''
            mkdir -p $out/lib $out/include
            cp .libs/libwinpthread.a $out/lib/
            cp .libs/libwinpthread.dll.a $out/lib/
            # Rust x86_64-pc-windows-gnu expects libpthread.a
            ln -s libwinpthread.a $out/lib/libpthread.a
            cp include/*.h $out/include/
          '';
        };
        localTargets = {
          opencode = agentLib.defaultLocalTargets.opencode // {
            enable = true;
          };
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
                targets = [ "x86_64-pc-windows-gnu" ];
              })
              pkg-config
              rustPlatform.bindgenHook
              # OpenSSL (required for native-tls)
              openssl
            ]
            ++ lib.optionals (lib.strings.hasInfix "linux" system) [
              # for Linux
              # Faster linker
              mold
              clang
              # Windows cross-compilation (MinGW)
              pkgs.pkgsCross.mingwW64.stdenv.cc
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
            # Windows cross-compilation environment
            # NOTE: Only use underscore-form env vars; dashed names (e.g. CC_x86_64-pc-windows-gnu)
            # are invalid in bash and will be silently corrupted.
            CC_x86_64_pc_windows_gnu = "${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}cc";
            CXX_x86_64_pc_windows_gnu = "${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}c++";
            AR_x86_64_pc_windows_gnu = "${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}ar";
            # MinGW compat header for aws-lc-sys (jitterentropy needs <sched.h>)
            CFLAGS_x86_64-pc-windows-gnu = "-idirafter ${mingwCompatHeaders}/include";
            CFLAGS_x86_64_pc_windows_gnu = "-idirafter ${mingwCompatHeaders}/include";
            # Libpthread for Rust x86_64-pc-windows-gnu target (via winpthreads)
            CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = "-L${mingwPthreads}/lib";
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
            shellHook =
              let
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
              in
              ''
                debug_log_path="/home/pexisgle/dev/Ene/.cursor/debug-7549ee.log"
                debug_ts="$(date +%s%3N 2>/dev/null || date +%s000)"
                is_interactive="false"
                if [[ "$-" == *i* ]]; then
                  is_interactive="true"
                fi
                #region agent log
                printf '%s\n' "{\"sessionId\":\"7549ee\",\"runId\":\"pre-fix\",\"hypothesisId\":\"H2\",\"location\":\"flake.nix:shellHook:entry\",\"message\":\"shellHook entry\",\"data\":{\"interactive\":\"$is_interactive\",\"shellFlags\":\"$-\"},\"timestamp\":$debug_ts}" >> "$debug_log_path"
                #endregion
                #region agent log
                printf '%s\n' "{\"sessionId\":\"7549ee\",\"runId\":\"pre-fix\",\"hypothesisId\":\"H1\",\"location\":\"flake.nix:shellHook:beforeSkillsHook\",\"message\":\"before skillsHook\",\"data\":{\"pwd\":\"$PWD\"},\"timestamp\":$(date +%s%3N 2>/dev/null || date +%s000)}" >> "$debug_log_path"
                #endregion
                if [[ "$is_interactive" == "true" ]]; then
                  #region agent log
                  printf '%s\n' "{\"sessionId\":\"7549ee\",\"runId\":\"post-fix\",\"hypothesisId\":\"H4\",\"location\":\"flake.nix:shellHook:interactiveBranch\",\"message\":\"running interactive-only hooks\",\"data\":{\"interactive\":\"true\"},\"timestamp\":$(date +%s%3N 2>/dev/null || date +%s000)}" >> "$debug_log_path"
                  #endregion
                  set +e
                  ${skillsHook}
                  skills_hook_rc="$?"
                  set -e
                  #region agent log
                  printf '%s\n' "{\"sessionId\":\"7549ee\",\"runId\":\"post-fix\",\"hypothesisId\":\"H1\",\"location\":\"flake.nix:shellHook:afterSkillsHook\",\"message\":\"after skillsHook\",\"data\":{\"skillsHookRc\":$skills_hook_rc},\"timestamp\":$(date +%s%3N 2>/dev/null || date +%s000)}" >> "$debug_log_path"
                  #endregion
                  ${appindicatorHook}
                else
                  #region agent log
                  printf '%s\n' "{\"sessionId\":\"7549ee\",\"runId\":\"post-fix\",\"hypothesisId\":\"H5\",\"location\":\"flake.nix:shellHook:nonInteractiveBranch\",\"message\":\"skipping interactive-only hooks\",\"data\":{\"interactive\":\"false\"},\"timestamp\":$(date +%s%3N 2>/dev/null || date +%s000)}" >> "$debug_log_path"
                  #endregion
                  skills_hook_rc=0
                fi
                #region agent log
                printf '%s\n' "{\"sessionId\":\"7549ee\",\"runId\":\"post-fix\",\"hypothesisId\":\"H3\",\"location\":\"flake.nix:shellHook:afterAppindicator\",\"message\":\"after appindicator hook\",\"data\":{\"ldLibraryPathSet\":\"true\",\"skillsHookRc\":$skills_hook_rc},\"timestamp\":$(date +%s%3N 2>/dev/null || date +%s000)}" >> "$debug_log_path"
                #endregion
              '';
          };
      }
    );
}

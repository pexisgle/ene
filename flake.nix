{
  description = "Ene assistant development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        inherit (pkgs) lib;

        # Create compatibility symlinks for crates expecting standard libappindicator3.
        # This replaces running dynamic shell commands on startup (mktemp / ln -sf).
        appindicatorCompat = pkgs.runCommand "appindicator-compat" { } ''
          mkdir -p $out/lib
          ln -sfn ${pkgs.libayatana-appindicator}/lib/libayatana-appindicator3.so.1 $out/lib/libappindicator3.so.1
          ln -sfn ${pkgs.libayatana-appindicator}/lib/libayatana-appindicator3.so.1 $out/lib/libappindicator3.so
        '';

        isLinux = lib.strings.hasInfix "linux" system;

        # Shared shell for local/dev (`default`) and a slim variant (`ci`)
        # without Windows cross, Chromium, sccache, and git-cliff. GitHub
        # Actions provisions these dependencies via apt, not this shell.
        mkEneShell =
          {
            withCrossWindows ? false,
            withChromium ? false,
            withSccache ? false,
            withGitCliff ? false,
            withVulkanTools ? false,
          }:
          pkgs.mkShell (
            {
              buildInputs =
                [
                  (pkgs.rust-bin.stable.latest.default.override {
                    extensions = [
                      "rust-src"
                      "clippy"
                    ];
                    targets = lib.optionals withCrossWindows [ "x86_64-pc-windows-gnu" ];
                  })
                  pkgs.pkg-config
                  pkgs.rustPlatform.bindgenHook
                  pkgs.openssl
                ]
                ++ lib.optionals withGitCliff [ pkgs.git-cliff ]
                ++ lib.optionals withSccache [ pkgs.sccache ]
                ++ lib.optionals isLinux (
                  [
                    pkgs.mold
                    pkgs.clang
                    pkgs.cmake
                    pkgs.alsa-lib
                    pkgs.libayatana-appindicator
                    pkgs.mesa
                    pkgs.vulkan-loader
                    pkgs.vulkan-headers
                    pkgs.shaderc
                    pkgs.libudev-zero
                    pkgs.libgbm
                    pkgs.libx11
                    pkgs.libxcursor
                    pkgs.libXi
                    pkgs.libxrandr
                    pkgs.libclang
                    pkgs.pipewire
                    pkgs.wayland
                    pkgs.wayland-protocols
                    pkgs.glib
                    pkgs.pango
                    pkgs.cairo
                    pkgs.gdk-pixbuf
                    pkgs.gtk3
                    pkgs.libxkbcommon
                    pkgs.xdotool
                  ]
                  ++ lib.optionals withCrossWindows [ pkgs.pkgsCross.mingwW64.stdenv.cc ]
                  ++ lib.optionals withVulkanTools [ pkgs.vulkan-tools ]
                  ++ lib.optionals withChromium [ pkgs.chromium ]
                );

              RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
              LD_LIBRARY_PATH = lib.makeLibraryPath (
                [
                  pkgs.mesa
                  pkgs.libgbm
                  pkgs.vulkan-loader
                  pkgs.libx11
                  pkgs.libXi
                  pkgs.libxcursor
                  pkgs.libxkbcommon
                  pkgs.xdotool
                ]
                ++ lib.optionals isLinux [
                  pkgs.libayatana-appindicator
                  appindicatorCompat
                ]
              );
              LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

              OPENSSL_DIR = "${pkgs.openssl.dev}";
              OPENSSL_LIB_DIR = "${pkgs.openssl.out}/lib";
              OPENSSL_INCLUDE_DIR = "${pkgs.openssl.dev}/include";

              NIX_LDFLAGS = "-L${pkgs.xdotool}/lib";

              shellHook =
                ''
                  export CARGO_TARGET_DIR="$PWD/target"
                  export LIBSQLITE3_FLAGS="-DSQLITE_ENABLE_MATH_FUNCTIONS"
                ''
                + (
                  if withSccache then
                    ''
                      export RUSTC_WRAPPER="${pkgs.sccache}/bin/sccache"
                      export SCCACHE_DIR="$HOME/.cache/sccache"
                    ''
                  else
                    # .cargo/config.toml sets build.rustc-wrapper = "sccache";
                    # clear it so CI does not require the binary on PATH.
                    ''
                      export CARGO_BUILD_RUSTC_WRAPPER=""
                      unset RUSTC_WRAPPER
                    ''
                );
            }
            // lib.optionalAttrs withCrossWindows {
              CC_x86_64_pc_windows_gnu = "${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}cc";
              CXX_x86_64_pc_windows_gnu = "${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}c++";
              AR_x86_64_pc_windows_gnu = "${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}ar";
              CFLAGS_x86_64-pc-windows-gnu = "-idirafter ${pkgs.pkgsCross.mingwW64.windows.pthreads}/include";
              CFLAGS_x86_64_pc_windows_gnu = "-idirafter ${pkgs.pkgsCross.mingwW64.windows.pthreads}/include";
              CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = "-L${pkgs.pkgsCross.mingwW64.windows.pthreads}/lib";
            }
          );
      in
      {
        devShells.default = mkEneShell {
          withCrossWindows = true;
          withChromium = true;
          withSccache = true;
          withGitCliff = true;
          withVulkanTools = true;
        };

        devShells.ci = mkEneShell { };
      }
    );
}

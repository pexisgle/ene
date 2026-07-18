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
              cmake
              pkgs.pkgsCross.mingwW64.stdenv.cc
              alsa-lib
              libayatana-appindicator
              mesa
              vulkan-loader
              vulkan-headers
              vulkan-tools
              shaderc
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

            shellHook = ''
              export CARGO_TARGET_DIR="$PWD/target"
            '';
          };
      }
    );
}

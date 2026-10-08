{
  description = "Techne: a live, programmable environment and its Lisp";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # Test suites run by crates/techne-vm/tests/suites.rs, pinned here.
    chibi-scheme = { url = "github:ashinn/chibi-scheme"; flake = false; };
    r7rs-benchmarks = { url = "github:ecraven/r7rs-benchmarks"; flake = false; };
    # SRFI reference implementations, loaded unchanged with their tests.
    srfi-128 = { url = "github:scheme-requests-for-implementation/srfi-128"; flake = false; };
    srfi-133 = { url = "github:scheme-requests-for-implementation/srfi-133"; flake = false; };
    srfi-151 = { url = "github:scheme-requests-for-implementation/srfi-151"; flake = false; };
  };

  outputs = { self, nixpkgs, chibi-scheme, r7rs-benchmarks, srfi-128, srfi-133, srfi-151 }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in {
      devShells = forAll (pkgs:
        let
          llvm = pkgs.rustc.llvmPackages.llvm;
          rust = with pkgs; [ cargo rustc clippy rustfmt rust-analyzer cargo-nextest cargo-llvm-cov ];
          # Libraries the editor window (crates/techne-window) loads at run time.
          windowLibs = with pkgs; [ vulkan-loader libxkbcommon wayland libGL ];
          # Libraries for the compositor (crates/techne-compositor).
          compositorLibs = with pkgs; [
            libxkbcommon libGL wayland libx11 libxcursor libxrandr libxi
            seatd libinput systemd libdrm libgbm
          ];
        in {
          # The language, tools and tests.
          default = pkgs.mkShell {
            packages = rust ++ (with pkgs; [
              valgrind   # instruction counts for benchmarks (runtime/bench/icount.sh)
              python3    # generates the orgparse benchmark's input
              hyperfine  # wall-clock comparisons on a quiet machine
              chez       # reference implementation for differential tests
              typos      # spelling (_typos.toml)
            ]);
            TECHNE_R7RS_TESTS = "${chibi-scheme}/tests/r7rs-tests.scm";
            TECHNE_R7RS_BENCHMARKS = "${r7rs-benchmarks}";
            # One directory per SRFI reference implementation (srfi-128, ...).
            TECHNE_SRFI_SOURCES = pkgs.linkFarm "srfi-sources" [
              { name = "srfi-128"; path = srfi-128; }
              { name = "srfi-133"; path = srfi-133; }
              { name = "srfi-151"; path = srfi-151; }
            ];
            # cargo-llvm-cov needs the LLVM that rustc was built with.
            LLVM_COV = "${llvm}/bin/llvm-cov";
            LLVM_PROFDATA = "${llvm}/bin/llvm-profdata";
          };

          # Everything above plus what running the editor window needs, and a
          # headless Wayland session to run it in (crates/techne-window/headless.sh).
          window = pkgs.mkShell {
            packages = rust ++ (with pkgs; [ sway grim wtype ]);
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath windowLibs;
          };

          # Everything above plus the compositor's system libraries.
          compositor = pkgs.mkShell {
            packages = rust ++ (with pkgs; [ pkg-config ]);
            buildInputs = compositorLibs ++ (with pkgs; [
              glib.dev seatd.dev libinput.dev systemd.dev libdrm.dev
              libdisplay-info_0_3 pipewire.dev llvmPackages.libclang.lib
            ]);
            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
            BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.glibc.dev}/include";
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath compositorLibs;
          };
        });
    };
}

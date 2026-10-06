{
  description = "Techne: a live, programmable environment and its Lisp";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # Test suites run by crates/techne-vm/tests/suites.rs, pinned here.
    chibi-scheme = { url = "github:ashinn/chibi-scheme"; flake = false; };
    r7rs-benchmarks = { url = "github:ecraven/r7rs-benchmarks"; flake = false; };
  };

  outputs = { self, nixpkgs, chibi-scheme, r7rs-benchmarks }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in {
      devShells = forAll (pkgs:
        let
          llvm = pkgs.rustc.llvmPackages.llvm;
          rust = with pkgs; [ cargo rustc clippy rustfmt rust-analyzer cargo-nextest cargo-llvm-cov ];
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
              hyperfine  # wall-clock comparisons on a quiet machine
              chez       # reference implementation for differential tests
            ]);
            TECHNE_R7RS_TESTS = "${chibi-scheme}/tests/r7rs-tests.scm";
            TECHNE_R7RS_BENCHMARKS = "${r7rs-benchmarks}";
            # cargo-llvm-cov needs the LLVM that rustc was built with.
            LLVM_COV = "${llvm}/bin/llvm-cov";
            LLVM_PROFDATA = "${llvm}/bin/llvm-profdata";
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

{
  description = "assertion-executor";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };
  
  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachSystem [ "aarch64-linux" "aarch64-darwin" "x86_64-linux" ] (system:
      let
        overlays = [ rust-overlay.overlays.default ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        lib = pkgs.lib;
        cargoMeta = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            pkg-config
            cargo-flamegraph
            cargo-fuzz
            cargo-tarpaulin
            lldb
            (rust-bin.nightly.latest.default.override {
              extensions = [ "rust-src" "rustfmt-preview" "rust-analyzer" ];
            })
          ] ++ lib.optional pkgs.stdenv.isLinux [
            pkgs.cargo-llvm-cov
            pkgs.llvmPackages_12.stdenv
            pkgs.clang_18
            pkgs.valgrind
            pkgs.gdb
            pkgs.openssl.dev
            pkgs.linuxPackages_latest.perf
          ] ++ lib.optionals stdenv.isDarwin [
            darwin.apple_sdk.frameworks.SystemConfiguration
          ];

          shellHook = ''
            export CARGO_BUILD_RUSTC_WRAPPER=$(which sccache)
            export RUSTC_WRAPPER=$(which sccache)
            export OLD_PS1="$PS1" # Preserve the original PS1
            export PS1="nix-shell:assertion-executor $PS1"
          '' 
          + lib.optionalString pkgs.stdenv.isLinux
          ''
            # For generating code coverage reports using `cargo-llvm-cov`
            export LLVM_COV=${pkgs.llvmPackages.llvm}/bin/llvm-cov
            export LLVM_PROFDATA=${pkgs.llvmPackages.llvm}/bin/llvm-profdata
            # Linux-specific C compiler setup
            export LIBCLANG_PATH="${pkgs.llvmPackages_12.libclang.lib}/lib"
            # Ensure C/C++ compilers can find headers and libraries
            export C_INCLUDE_PATH=${pkgs.glibc.dev}/include:${pkgs.llvmPackages.libclang.lib}/lib/clang/${pkgs.llvmPackages.libclang.version}/include:$C_INCLUDE_PATH
            export CPLUS_INCLUDE_PATH=${pkgs.glibc.dev}/include:${pkgs.llvmPackages.libclang.lib}/lib/clang/${pkgs.llvmPackages.libclang.version}/include:$CPLUS_INCLUDE_PATH
            export LIBRARY_PATH=${pkgs.glibc}/lib:${pkgs.llvmPackages.libclang.lib}/lib:$LIBRARY_PATH
            export LD_LIBRARY_PATH=${pkgs.glibc}/lib:${pkgs.llvmPackages.libclang.lib}/lib:$LD_LIBRARY_PATH
          '';

          # reset ps1
          shellExitHook = ''
            export PS1="$OLD_PS1"
          '';
        };
      }
    );
}
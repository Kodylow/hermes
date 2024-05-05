{
  inputs = {
    nixpkgs = { url = "github:nixos/nixpkgs/nixos-23.11"; };

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flakebox = {
      url = "github:dpc/flakebox?rev=226d584e9a288b9a0471af08c5712e7fac6f87dc";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.fenix.follows = "fenix";
    };

    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, fenix, flakebox, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        setupPostgresScript = pkgs.writeShellScript "setup-postgres" ''
                    export PGDATA=$(mktemp -d)
                    export PGSOCKETS=$(mktemp -d)
                    ${pkgs.postgresql}/bin/initdb -D $PGDATA
                    ${pkgs.postgresql}/bin/pg_ctl start -D $PGDATA -o "-h localhost -p 5432 -k $PGSOCKETS"
                    until ${pkgs.postgresql}/bin/pg_isready -h localhost -p 5432; do sleep 1; done
                    ${pkgs.postgresql}/bin/createuser -h localhost -p 5432 -s postgres
                    ${pkgs.postgresql}/bin/psql -h localhost -p 5432 -c "CREATE USER \"hermes_user\" WITH PASSWORD 'password';" -U postgres
                    ${pkgs.postgresql}/bin/psql -h localhost -p 5432 -c "CREATE DATABASE \"hermes\" OWNER \"hermes_user\";" -U postgres
          	  exit
        '';

        setupEnvScript = pkgs.writeShellScript "setup-env" ''
                    if [ ! -f .env ]; then
                      cp .env.sample .env
                      sed -i 's|DATABASE_URL=postgres://localhost/hermes|DATABASE_URL=postgres://hermes_user:password@localhost:5432/hermes|g' .env
          	    # random nsec for CI only
                      sed -i 's|NSEC=|NSEC=nsec1lmtupx60q0pg6lk3kcl0c56mp7xukulmcc2rxu3gd6sage8xzxhs3slpac|g' .env
          	    # localhost domain
                      sed -i 's|DOMAIN_URL=|DOMAIN_URL=http://127.0.0.1:8080|g' .env
                    fi
        '';

        setupFedimintTestDirScript =
          pkgs.writeShellScript "setup-fedimint-test-dir" ''
            if [ -d .fedimint-test-dir ]; then
              rm -rf .fedimint-test-dir
            fi
            mkdir -m 700 .fedimint-test-dir
          '';

        lib = pkgs.lib;
        flakeboxLib = flakebox.lib.${system} { };
        rustSrc = flakeboxLib.filterSubPaths {
          root = builtins.path {
            name = "hermes";
            path = ./.;
          };
          paths = [ "Cargo.toml" "Cargo.lock" ".cargo" "src" ];
        };
        toolchainArgs = let llvmPackages = pkgs.llvmPackages_11;
        in {
          extraRustFlags = "--cfg tokio_unstable";

          components = [ "rustc" "cargo" "clippy" "rust-analyzer" "rust-src" ];

          args = {
            nativeBuildInputs =
              [ pkgs.wasm-bindgen-cli pkgs.geckodriver pkgs.wasm-pack ]
              ++ lib.optionals (!pkgs.stdenv.isDarwin) [ ];
          };
        } // lib.optionalAttrs pkgs.stdenv.isDarwin {
          # on Darwin newest stdenv doesn't seem to work
          # linking rocksdb
          stdenv = pkgs.clang11Stdenv;
          clang = llvmPackages.clang;
          libclang = llvmPackages.libclang.lib;
          clang-unwrapped = llvmPackages.clang-unwrapped;
        };

        # all standard toolchains provided by flakebox
        toolchainsStd = flakeboxLib.mkStdFenixToolchains toolchainArgs;

        toolchainsNative = (pkgs.lib.getAttrs [ "default" ] toolchainsStd);

        toolchainNative =
          flakeboxLib.mkFenixMultiToolchain { toolchains = toolchainsNative; };

        commonArgs = {
          buildInputs = [
            pkgs.just
            pkgs.openssl
            pkgs.openssl.dev
            pkgs.zlib
            pkgs.postgresql
            pkgs.gcc
            pkgs.gcc.cc.lib
            pkgs.pkg-config
            pkgs.libclang.lib
            pkgs.clang
            pkgs.flyctl
          ] ++ lib.optionals pkgs.stdenv.isDarwin
            [ pkgs.darwin.apple_sdk.frameworks.SystemConfiguration ];
          nativeBuildInputs = [ pkgs.pkg-config ];
        };

        outputs = (flakeboxLib.craneMultiBuild { toolchains = toolchainsStd; })
          (craneLib':
            let
              craneLib = (craneLib'.overrideArgs {
                pname = "flexbox-multibuild";
                src = rustSrc;
              }).overrideArgs commonArgs;
            in rec {
              workspaceDeps = craneLib.buildWorkspaceDepsOnly { };
              workspaceBuild =
                craneLib.buildWorkspace { cargoArtifacts = workspaceDeps; };
              hermes = craneLib.buildPackageGroup {
                pname = "hermes";
                packages = [ "hermes" ];
                mainProgram = "hermes";
              };
            });
      in {
        legacyPackages = outputs;
        packages = { default = outputs.hermes; };
        devShells = flakeboxLib.mkShells {
          packages = [ ];
          buildInputs = commonArgs.buildInputs;
          nativeBuildInputs = [ ];
          shellHook = ''
            export RUSTFLAGS="--cfg tokio_unstable"
            export RUSTDOCFLAGS="--cfg tokio_unstable"
            export RUST_LOG="info"
            export LIBCLANG_PATH="${pkgs.libclang.lib}/lib"
            export LD_LIBRARY_PATH=${pkgs.openssl}/bin:${pkgs.gcc.cc.lib}/lib:$LD_LIBRARY_PATH
            export PKG_CONFIG_PATH=${pkgs.openssl.dev}/lib/pkgconfig

            ${setupPostgresScript}
            ${setupEnvScript}
            ${setupFedimintTestDirScript}
          '';
        };
      });
}

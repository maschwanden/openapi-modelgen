{
  description = "openapi-modelgen — generate Rust request and response types from OpenAPI 3.0 specs";

  inputs = {
    nixpkgs.url = "nixpkgs/nixos-25.11";

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      ...
    }@inputs:
    let
      supportedSystems = [
        "x86_64-linux"
        "x86_64-darwin"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;

      nixpkgsFor = forAllSystems (
        system:
        import nixpkgs {
          inherit system;
          overlays = [ inputs.fenix.overlays.default ];
        }
      );
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgsFor.${system};
          toolchain = pkgs.fenix.stable;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = toolchain.cargo;
            rustc = toolchain.rustc;
          };
        in
        {
          default = rustPlatform.buildRustPackage {
            pname = "openapi-modelgen";
            version = "0.1.2";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
          };
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/openapi-modelgen";
        };
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgsFor.${system};
        in
        {
          default = pkgs.mkShell {
            packages = [
              (pkgs.fenix.stable.withComponents [
                "cargo"
                "clippy"
                "rust-src"
                "rustc"
                "rustfmt"
              ])
              pkgs.fenix.stable.rust-analyzer

              pkgs.nixfmt-tree
            ];
          };
        }
      );
    };
}

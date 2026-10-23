{

  description = "nix-output-monitor drawn natively in Tern";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    pedantix = {
      inputs = {
        nixpkgs.follows = "nixpkgs";
        treefmt-nix.follows = "treefmt-nix";
      };

      url = "github:swarsel/pedantix";
    };

    snowfall-lib = {
      inputs.nixpkgs.follows = "nixpkgs";
      url = "github:anntnzrb/snowfall-lib";
    };

    treefmt-nix = {
      inputs.nixpkgs.follows = "nixpkgs";
      url = "github:numtide/treefmt-nix";
    };
  };

  outputs =
    inputs:
    inputs.snowfall-lib.mkFlake {
      inherit inputs;
      alias.packages.default = "tern-nom";

      outputs-builder =
        channels:
        let
          treefmt = inputs.treefmt-nix.lib.evalModule channels.nixpkgs {
            imports = [ inputs.pedantix.treefmtModules.default ];

            programs = {
              pedantix = {
                enable = true;

                settings = {
                  attrs = {
                    blank-lines = 1;
                    blank-lines-mode = "multiline";
                    flatten = true;
                    merge = true;
                    name-style = "identifier";
                  };

                  formatter = "nixfmt";
                  inherit-placement = "front";
                  inherits.name-style = "identifier";
                  lets.name-style = "identifier";
                  lists.sort = false;
                };
              };

              rustfmt.enable = true;
            };

            projectRootFile = "flake.nix";
          };
        in
        {
          checks.formatting = treefmt.config.build.check inputs.self;
          formatter = treefmt.config.build.wrapper;
        };

      snowfall = {
        namespace = "tern-nom";
        root = ./nix;
      };

      src = ./.;

      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
    };
}

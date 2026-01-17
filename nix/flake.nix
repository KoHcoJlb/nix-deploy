{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs";
    sops-nix = {
      url = "github:Mic92/sops-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, sops-nix, ... }:
    let
      lib = nixpkgs.lib.extend (
        _: _: {
          noChroot =
            x:
            x.overrideAttrs {
              outputHash = null;
              __noChroot = true;
            };
        }
      );
    in
    {
      init =
        {
          inputs,
          hosts,
          values,
        }:

        with lib;
        let
          hostSystem = "x86_64-linux";

          makeSystemFn =
            name: config: modules:
            nixosSystem {
              specialArgs = {
                inherit inputs values;

                systems = filterAttrs (_: system: !system.deploy.skip) (
                  mapAttrs (_: system: system.config) systemsUnmerged
                );
              };
              modules = [
                ./deploy.nix
                sops-nix.nixosModules.sops
                config
                {
                  nixpkgs = {
                    buildPlatform = mkDefault hostSystem;
                    hostPlatform = mkDefault hostSystem;

                    overlays = [
                      (_: _: {
                        inherit lib;
                      })
                    ];
                  };
                  networking = {
                    hostName = name;
                  };
                }
              ]
              ++ modules;
            };

          systems = mapAttrs makeSystemFn hosts;

          systemsUnmerged = mapAttrs (_: systemFn: systemFn [ ]) systems;

          systemsMerged = mapAttrs (
            name: systemFn:
            systemFn (
              mapAttrsToList (_: system: {
                deploy.global = {
                  imports = map (def: {
                    _file = def.file;
                    imports = [ def.value ];
                  }) system.options.deploy.global.definitionsWithLocations;
                };
              }) (removeAttrs systemsUnmerged [ name ])
            )
          ) systems;

        in
        {
          inherit systemsUnmerged;
          nixosConfigurations = systemsMerged;
          systemNames = attrNames systems;

          systemMetadata = mapAttrs (
            name: system:
            let
              deployCfg = system.config.deploy;
            in
            {
              inherit name;
              inherit (deployCfg) targetHost tags skip;
              sopsFiles = mapAttrsToList (_: secret: secret.sopsFile) system.config.sops.secrets;
            }
          ) systemsUnmerged;
        };
    };
}

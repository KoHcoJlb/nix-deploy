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
        final: _: {
          formatHost =
            host: if final.hasInfix ":" host && !final.hasPrefix "[" host then "[${host}]" else host;

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
          systems,
          values,
          domain ? null,
          exportModules ? [ ],
        }:

        with lib;
        let
          hostPlatform = "x86_64-linux";

          initializedSystems = mapAttrs (
            name: definition:
            (evalModules {
              prefix = [
                "systems"
                name
              ];
              specialArgs = {
                inherit
                  lib
                  inputs
                  values
                  name
                  ;
              };
              modules = [
                ./system.nix
                { domain = mkDefault domain; }
                (setDefaultModuleLocation ((builtins.unsafeGetAttrPos name systems).file or "<unknown-file>"
                ) definition)
              ];
            }).config
          ) systems;

          sharedExports = evalModules {
            specialArgs = { inherit inputs values; };
            modules = exportModules ++ mapAttrsToList (_: system: system.exports) initializedSystems;
          };

          makeSystem =
            name: system:
            nixosSystem {
              specialArgs = {
                inherit inputs values system;
                globalExports = sharedExports.config;
              };
              modules = [
                sops-nix.nixosModules.sops
                system.nixosModule
                {
                  nixpkgs = {
                    buildPlatform = mkDefault hostPlatform;
                    hostPlatform = mkDefault hostPlatform;

                    overlays = [
                      (_: _: {
                        inherit lib;
                      })
                    ];
                  };
                  networking = {
                    hostName = mkDefault name;
                    domain = mkDefault system.domain;
                  };
                }
              ];
            };

          nixosConfigurations = mapAttrs makeSystem initializedSystems;

        in
        {
          inherit nixosConfigurations;

          exports = sharedExports.config;
          systems = initializedSystems;

          systemNames = attrNames initializedSystems;
          systemMetadata = mapAttrs (
            name: system:
            let
              # Skip unknown-option checking only for metadata; builds use nixosConfigurations unchanged.
              config =
                (nixosConfigurations.${name}.extendModules {
                  modules = [ { _module.check = mkForce false; } ];
                }).config;
            in
            {
              inherit name;
              inherit (system) targetHost targetPort skip;
              tags = system.tags ++ [ config.nixpkgs.hostPlatform.linuxArch ];
              sopsFiles = mapAttrsToList (_: secret: secret.sopsFile) config.sops.secrets;
            }
          ) initializedSystems;
        };
    };
}

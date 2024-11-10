{
  inputs = {};

  outputs = { ... }: {
    init = { inputs, hosts, values }: with inputs.nixpkgs.lib; let
      hostSystem = "x86_64-linux";
      pkgs = inputs.nixpkgs.legacyPackages.${hostSystem};

      makeSystemFn = name: config: modules: nixosSystem {
        specialArgs = { 
          inherit inputs values;
          systems = ./configurations/systems;
        };
        modules = [
          ./deploy.nix
          config
          {
            nixpkgs = {
              buildPlatform = hostSystem;
              hostPlatform = mkDefault hostSystem;
            };
            networking = {
              hostName = name;
            };
          }
        ] ++ modules;
      };

      systems = mapAttrs
        makeSystemFn
        (import hosts { inherit inputs; });

      systemsUnmerged = mapAttrs (_: systemFn: systemFn []) systems;

      systemsMerged = mapAttrs (name: systemFn: 
        systemFn (mapAttrsToList (_: system: {
          deploy.global = {
            imports = map (def: {
              _file = def.file;
              imports = [def.value];
            }) system.options.deploy.global.definitionsWithLocations;
          };
        }) (removeAttrs systemsUnmerged [name]))
      ) systems;

    in {
      nixosConfigurations = systemsMerged;

      toplevel = mapAttrs (_: system: system.config.system.build.toplevel) systemsMerged;

      targetHost = mapAttrs (_: system:
        system.config.deploy.targetHost
      ) systemsUnmerged;

      systemNames = attrNames systems;

      apps.${hostSystem}.deploy = {
        type = "app";
        program = toString (pkgs.writers.writePython3 "deploy.py" { 
          doCheck = false;
          libraries = with pkgs.python3Packages; [ ruamel-yaml ];
        } ./tools.py);
      };
    };
  };
}
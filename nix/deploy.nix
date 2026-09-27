{ lib, config, ... }:

with lib;

{
  options = {
    deploy = {
      global = mkOption {
        type = types.submoduleWith {
          modules = [
            {
              freeformType = types.attrsOf types.anything;
            }
          ];
        };
        default = { };
      };

      targetHost = mkOption {
        type = types.str;
        default = "${config.networking.fqdn}";
      };

      targetPort = mkOption {
        type = types.port;
        default = 22;
        description = "SSH port used to connect to the deployment target.";
      };

      tags = mkOption {
        type = types.listOf types.str;
        default = [ ];
      };

      skip = mkOption {
        type = types.bool;
        default = false;
      };
    };
  };

  config = {
    deploy.tags = [ config.nixpkgs.hostPlatform.linuxArch ];
  };
}

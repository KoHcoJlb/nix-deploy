{ lib, config, ... }:

with lib;

{
  options = {
    deploy = {
      global = mkOption {
        type = types.submoduleWith {
          modules = [];
        };
        default = {};
      };

      targetHost = mkOption {
        type = types.str;
        default = "${config.networking.fqdn}";
      };

      tags = mkOption {
        type = types.listOf types.str;
        default = [];
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

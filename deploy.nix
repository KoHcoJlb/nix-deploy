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
    };
  };
}

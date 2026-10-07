{
  config,
  lib,
  name,
  ...
}:

{
  options = {
    name = lib.mkOption {
      type = lib.types.str;
      default = name;
      readOnly = true;
      description = "System name derived from the inventory key.";
    };

    domain = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "System domain, or null for an unqualified name.";
    };
    fqdn = lib.mkOption {
      type = lib.types.str;
      default = if config.domain == null then config.name else "${config.name}.${config.domain}";
      readOnly = true;
      description = "System name qualified by its domain.";
    };

    targetHost = lib.mkOption {
      type = lib.types.str;
      default = config.fqdn;
      defaultText = lib.literalExpression "config.fqdn";
      description = "Raw deployment and cross-system connection address.";
    };
    targetHost' = lib.mkOption {
      type = lib.types.str;
      default = lib.formatHost config.targetHost;
      readOnly = true;
      description = "Target address formatted for host-and-port and URL rendering.";
    };
    targetPort = lib.mkOption {
      type = lib.types.ints.between 1 65535;
      default = 22;
      description = "SSH port used for deployment.";
    };

    tags = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = "Deployment selection tags, in addition to the system architecture.";
    };

    skip = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Exclude this system from tag-based deployment selection.";
    };

    exports = lib.mkOption {
      type = lib.types.deferredModule;
      default = { };
      description = "Module contributing to the shared cross-system exports.";
    };

    nixosModule = lib.mkOption {
      type = lib.types.deferredModule;
      default = { };
      description = "Module evaluated as part of this system's NixOS configuration.";
    };
  };

  config._module.args.system = removeAttrs config [ "_module" ];
}

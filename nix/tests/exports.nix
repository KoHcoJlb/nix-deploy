# From the repository root: nix eval --impure --expr 'import ./nix/tests/exports.nix (builtins.getFlake (toString ./nix)).inputs'
{ nixpkgs, sops-nix }:

let
  inherit (nixpkgs) lib;
  deploy = (import ../flake.nix).outputs { inherit nixpkgs sops-nix; };
  schema = {
    options = {
      enabled = lib.mkOption {
        type = lib.types.bool;
        default = true;
      };

      records = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
      };
    };
  };
  capture = { system, globalExports, ... }: {
    options.captured = lib.mkOption { type = lib.types.raw; };
    config = {
      captured = { inherit system globalExports; };
    };
  };
  exported =
    system:
    (lib.evalModules {
      modules = [
        schema
        system.exports
      ];
    }).config;
  evaluate =
    systems:
    deploy.init {
      inputs.marker = "from-inputs";
      values.domain = "example.test";
      domain = "example.test";
      exportModules = [ schema ];
      inherit systems;
    };
  fails = value: !(builtins.tryEval (builtins.deepSeq value true)).success;

  selfSystem = { system, lib, ... }: {
    options.port = lib.mkOption {
      type = lib.types.port;
      default = 8080;
    };
    config = {
      domain = "${system.name}.test";
      targetHost = "ssh.${system.fqdn}";
      exports.records = [
        system.name
        system.domain
        system.fqdn
        system.targetHost
        (toString system.port)
      ];
      nixosModule = _: throw "final system access evaluated NixOS";
    };
  };
  selfReferenced = evaluate { node = selfSystem; };
  selfOverridden = evaluate {
    renamed = {
      imports = [ selfSystem ];
      domain = lib.mkForce "media.test";
      targetHost = lib.mkForce "192.0.2.10";
      port = 9090;
      nixosModule = lib.mkForce capture;
    };
  };

  systems = {
    producer = _: {
      exports = { config, lib, ... }: {
        records = lib.mkMerge [
          (lib.mkBefore [ "first" ])
          (lib.mkIf config.enabled (lib.mkAfter [ "last" ]))
        ];
      };
      nixosModule = _: throw "export discovery evaluated the producer";
    };
    consumer = {
      options.custom.message = lib.mkOption { type = lib.types.str; };
      config = {
        custom.message = "system-specific data";
        exports.records = [ "middle" ];
        nixosModule = capture;
      };
    };
  };
  valid = evaluate systems;
  renamed = evaluate {
    renamed =
      {
        system,
        lib,
        inputs,
        values,
        ...
      }:
      {
        imports = [ systems.consumer ];
        exports = lib.mkForce { records = [ (lib.toUpper system.name) ]; };
        targetHost = "${system.name}.${values.domain}";
        tags = [ inputs.marker ];
      };
  };
  configured = evaluate {
    consumer = {
      imports = [ systems.consumer ];
      targetHost = "ssh.example.test";
      targetPort = 2222;
      tags = [ "custom" ];
      skip = true;
    };
  };
  invalidType = evaluate {
    broken = {
      exports.records = [ 42 ];
      nixosModule = capture;
    };
  };
  unknownExport = evaluate {
    broken = {
      exports.typo = true;
      nixosModule = { };
    };
  };
  invalidSystem = evaluate {
    broken.nixosModule = {
      _module.check = true;
      notARealNixOSOption = true;
      sops.secrets.example.sopsFile = ./exports.nix;
    };
  };
  priority = evaluate {
    defaulted = {
      exports.records = lib.mkDefault [ "default" ];
      nixosModule = capture;
    };
    forced = {
      exports.records = lib.mkForce [ "forced" ];
      nixosModule = capture;
    };
  };
  systemFile = builtins.toFile "nix-deploy-system.nix" ''
    { system, lib, inputs, values, ... }:
    {
      exports.records = [ (lib.toUpper system.name) ];
      targetHost = system.name + "." + values.domain;
      tags = [ inputs.marker ];
      nixosModule = { networking.domain = values.domain; };
    }
  '';
  fromPaths = evaluate {
    from-file = /. + builtins.unsafeDiscardStringContext systemFile;
    from-string = systemFile;
  };
  inventoryFile = builtins.toFile "nix-deploy-inventory.nix" ''
    {
      inline.exports.records = [ "inline" ];
      function = _: { exports.records = [ "function" ]; };
      imported.imports = [ ${systemFile} ];
      explicit = {
        _file = "explicit-system.nix";
        exports.records = [ "explicit" ];
      };
    }
  '';
  attributed = deploy.init {
    inputs = { };
    values.domain = "example.test";
    systems = import inventoryFile;
    exportModules = [
      schema
      ({ lib, options, ... }: {
        options.sources = lib.mkOption { type = lib.types.attrsOf lib.types.str; };
        config.sources = builtins.listToAttrs (
          map (definition: {
            name = lib.head definition.value;
            value = definition.file;
          }) options.records.definitionsWithLocations
        );
      })
    ];
  };
  unusedFile = builtins.toFile "unused-system.nix" ''throw "listing names imported a system"'';
  factsSystem = { system, lib, ... }: {
    options.custom.receivedDomain = lib.mkOption { type = lib.types.nullOr lib.types.str; };
    config = {
      custom.receivedDomain = system.domain;
      exports.records = [
        system.fqdn
        system.targetHost
        system.targetHost'
      ];
      nixosModule = capture;
    };
  };
  addressed = evaluate {
    node = {
      imports = [ factsSystem ];
      domain = "media.test";
      targetHost = "2001:db8::42";
    };
  };
  withoutDomain = deploy.init {
    inputs = { };
    values = { };
    systems.node = factsSystem;
    exportModules = [ schema ];
  };
  moduleOverrides = evaluate {
    node.nixosModule = {
      imports = [ capture ];

      networking = {
        hostName = "module-name";
        domain = "module.test";
      };
    };
  };
  templated = evaluate {
    container = {
      domain = "containers.test";
      nixosModule = ../templates/proxmox/lxc.nix;
    };
  };
  composed = evaluate {
    node = { config, lib, ... }: {
      imports = [
        ({ system, lib, ... }: {
          options.port = lib.mkOption {
            type = lib.types.port;
            default = 80;
          };
          config = {
            port = lib.mkDefault 8080;
            tags = [ "first" ];
            exports.records = [ "${system.name}:${toString system.port}" ];
            nixosModule.environment.variables.SUB_PORT = toString system.port;
          };
        })
        ({ system, lib, ... }: {
          tags = lib.mkAfter [ "second" ];
          exports.records = lib.mkAfter [ system.targetHost' ];
          nixosModule = { config, ... }: {
            environment.variables.SUB_NAME = config.networking.hostName;
          };
        })
      ];

      port = lib.mkForce 4242;
      targetHost = "2001:db8::42";
      skip = lib.mkIf (config.port == 4242) true;
      exports.records = lib.mkBefore [ "parent" ];
      nixosModule.imports = [ capture ];
    };
  };
  moduleFile = builtins.toFile "nix-deploy-module.nix" ''
    { config, pkgs, system, ... }:
    {
      environment.variables = {
        MODULE_HOSTNAME = config.networking.hostName;
        INVENTORY_NAME = system.name;
        MODULE_SYSTEM = pkgs.stdenv.hostPlatform.system;
      };
    }
  '';
  explicitModules = evaluate {
    attr-module.nixosModule = {
      imports = [ capture ];
    };
    simple-module.nixosModule = { lib, ... }: {
      imports = [ capture ];
      boot.isContainer = lib.mkDefault true;
    };
    function-module.nixosModule = import moduleFile;
    path-module.nixosModule = /. + builtins.unsafeDiscardStringContext moduleFile;
    string-module.nixosModule = moduleFile;
    optional-module.nixosModule =
      {
        pkgs ? null,
        ...
      }:
      {
        environment.variables.MODULE_SYSTEM =
          if pkgs == null then "missing module arguments" else pkgs.stdenv.hostPlatform.system;
      };
    optional-context-module.nixosModule =
      {
        globalExports ? throw "missing NixOS context",
        ...
      }:
      assert globalExports.enabled;
      {
        imports = [ capture ];
      };
    custom-arg-module.nixosModule = { customMarker, ... }: {
      _module.args.customMarker = "from-module-args";
      environment.variables.MODULE_MARKER = customMarker;
    };
    optional-custom-arg-module.nixosModule =
      {
        customMarker ? "probe-default",
        ...
      }:
      {
        _module.args.customMarker = "from-module-args";
        environment.variables.MODULE_MARKER = customMarker;
      };
    deferred-module.nixosModule = { config, ... }: throw "export discovery evaluated a NixOS module";
  };

in
assert
  selfReferenced.exports.records == [
    "node"
    "node.test"
    "node.node.test"
    "ssh.node.node.test"
    "8080"
  ];
assert
  selfOverridden.exports.records == [
    "renamed"
    "media.test"
    "renamed.media.test"
    "192.0.2.10"
    "9090"
  ];
assert
  (exported selfOverridden.nixosConfigurations.renamed.config.captured.system).records
  == selfOverridden.exports.records;
assert selfOverridden.systems.renamed.targetHost' == "192.0.2.10";
assert
  valid.systemNames == [
    "consumer"
    "producer"
  ];
assert
  valid.exports.records == [
    "first"
    "middle"
    "last"
  ];
assert valid.nixosConfigurations.consumer.config.captured.globalExports == valid.exports;
assert
  valid.nixosConfigurations.consumer.config.captured.system.custom.message == "system-specific data";
assert (exported valid.nixosConfigurations.consumer.config.captured.system).records == [ "middle" ];
assert valid.nixosConfigurations.consumer.config.captured.system.name == "consumer";
assert valid.systems.consumer.domain == "example.test";
assert valid.systems.consumer.fqdn == "consumer.example.test";
assert valid.systems.consumer.targetHost == "consumer.example.test";
assert valid.nixosConfigurations.consumer.config.networking.hostName == "consumer";
assert valid.nixosConfigurations.consumer.config.networking.domain == "example.test";
assert
  addressed.exports.records == [
    "node.media.test"
    "2001:db8::42"
    "[2001:db8::42]"
  ];
assert addressed.systems.node.custom.receivedDomain == "media.test";
assert addressed.systemMetadata.node.targetHost == "2001:db8::42";
assert addressed.nixosConfigurations.node.config.captured.system.targetHost' == "[2001:db8::42]";
assert
  (evaluate {
    node = {
      imports = [ systems.consumer ];
      targetHost = "[2001:db8::42]";
    };
  }).systems.node.targetHost' == "[2001:db8::42]";
assert addressed.nixosConfigurations.node.config.networking.domain == "media.test";
assert withoutDomain.systems.node.domain == null;
assert
  withoutDomain.exports.records == [
    "node"
    "node"
    "node"
  ];
assert withoutDomain.systemMetadata.node.targetHost == "node";
assert builtins.deepSeq withoutDomain.systemMetadata.node true;
assert withoutDomain.nixosConfigurations.node.config.networking.domain == null;
assert lib.all
  (
    case:
    let
      result = evaluate {
        node = {
          imports = [ factsSystem ];
          inherit (case) domain;
        };
      };
    in
    result.exports.records == [
      case.fqdn
      case.fqdn
      case.fqdn
    ]
    && result.systems.node.fqdn == case.fqdn
    && result.systemMetadata.node.targetHost == case.fqdn
    && result.nixosConfigurations.node.config.networking.domain == case.domain
  )
  [
    {
      domain = null;
      fqdn = "node";
    }
    {
      domain = "";
      fqdn = "node.";
    }
    {
      domain = "other.test";
      fqdn = "node.other.test";
    }
  ];
assert moduleOverrides.nixosConfigurations.node.config.networking.hostName == "module-name";
assert moduleOverrides.nixosConfigurations.node.config.networking.domain == "module.test";
assert moduleOverrides.systems.node.fqdn == "node.example.test";
assert moduleOverrides.systemMetadata.node.targetHost == "node.example.test";
assert templated.nixosConfigurations.container.config.networking.domain == "containers.test";
assert templated.systemMetadata.container.targetHost == "container.containers.test";
assert fails
  (evaluate {
    bare = {
      networking.hostName = "wrong-layer";
    };
  }).exports;
assert fails (evaluate { bare = _: { networking.hostName = "wrong-layer"; }; }).exports;
assert fails (evaluate { node.typo = true; }).systems.node;
assert lib.all (field: fails (evaluate { node.${field} = "stale-value"; }).systems.node.${field}) [
  "name"
  "fqdn"
  "targetHost'"
];
assert
  composed.exports.records == [
    "parent"
    "node:4242"
    "[2001:db8::42]"
  ];
assert composed.nixosConfigurations.node.config.environment.variables.SUB_PORT == "4242";
assert composed.nixosConfigurations.node.config.environment.variables.SUB_NAME == "node";
assert composed.nixosConfigurations.node.config.captured.system.port == 4242;
assert !(composed.nixosConfigurations.node.config.captured.system ? _module);
assert
  composed.systemMetadata.node.tags == [
    "first"
    "second"
    "x86_64"
  ];
assert composed.systemMetadata.node.skip;
assert builtins.deepSeq explicitModules.exports true;
assert explicitModules.exports.records == [ ];
assert explicitModules.systems.deferred-module.targetHost == "deferred-module.example.test";
assert explicitModules.nixosConfigurations.attr-module.config.captured.system.name == "attr-module";
assert explicitModules.nixosConfigurations.simple-module.config.boot.isContainer;
assert
  explicitModules.nixosConfigurations.optional-context-module.config.captured.system.name
  == "optional-context-module";
assert
  explicitModules.nixosConfigurations.optional-module.config.environment.variables.MODULE_SYSTEM
  == "x86_64-linux";
assert
  explicitModules.nixosConfigurations.custom-arg-module.config.environment.variables.MODULE_MARKER
  == "from-module-args";
assert
  explicitModules.nixosConfigurations.optional-custom-arg-module.config.environment.variables.MODULE_MARKER
  == "from-module-args";
assert fails explicitModules.nixosConfigurations.deferred-module.config.networking.hostName;
assert lib.all
  (
    name:
    let
      system = explicitModules.nixosConfigurations.${name};
    in
    system.config.environment.variables.MODULE_HOSTNAME == name
    && system.config.environment.variables.INVENTORY_NAME == name
    && system.config.environment.variables.MODULE_SYSTEM == "x86_64-linux"
    && explicitModules.systemMetadata.${name}.targetHost == "${name}.example.test"
  )
  [
    "function-module"
    "path-module"
    "string-module"
  ];
assert lib.any (definition: lib.hasPrefix moduleFile definition.file)
  explicitModules.nixosConfigurations.path-module.options.environment.variables.definitionsWithLocations;
assert builtins.isAttrs valid.nixosConfigurations.consumer.config.captured.system.nixosModule;
assert !(valid.nixosConfigurations.consumer.options ? deploy);
assert valid.nixosConfigurations.consumer._module.check;
assert valid.systemMetadata.consumer.targetHost == "consumer.example.test";
assert valid.systemMetadata.consumer.targetPort == 22;
assert !valid.systemMetadata.consumer.skip;
assert valid.systemMetadata.consumer.sopsFiles == [ ];
assert builtins.deepSeq valid.systemMetadata.consumer true;
assert renamed.systemNames == [ "renamed" ];
assert renamed.exports.records == [ "RENAMED" ];
assert renamed.systemMetadata.renamed.name == "renamed";
assert renamed.systemMetadata.renamed.targetHost == "renamed.example.test";
assert
  renamed.systemMetadata.renamed.tags == [
    "from-inputs"
    "x86_64"
  ];
assert renamed.nixosConfigurations.renamed.config.networking.hostName == "renamed";
assert renamed.nixosConfigurations.renamed.config.captured.system.name == "renamed";
assert
  (evaluate { unused = _: throw "listing names initialized a system"; }).systemNames == [ "unused" ];
assert (evaluate { unused = unusedFile; }).systemNames == [ "unused" ];
assert fails (evaluate { unused = unusedFile; }).exports;
assert lib.all
  (
    name:
    lib.elem (lib.toUpper name) fromPaths.exports.records
    && fromPaths.systemMetadata.${name}.name == name
    && fromPaths.systemMetadata.${name}.targetHost == "${name}.example.test"
    &&
      fromPaths.systemMetadata.${name}.tags == [
        "from-inputs"
        "x86_64"
      ]
    && fromPaths.nixosConfigurations.${name}.config.networking.hostName == name
  )
  [
    "from-file"
    "from-string"
  ];
assert lib.any (definition: lib.hasPrefix systemFile definition.file)
  (lib.evalModules {
    modules = [
      schema
      fromPaths.systems.from-file.exports
    ];
  }).options.records.definitionsWithLocations;
assert
  attributed.exports.sources == {
    inline = "${inventoryFile}, via option systems.inline.exports";
    function = "${inventoryFile}, via option systems.function.exports";
    IMPORTED = "${systemFile}, via option systems.imported.exports";
    explicit = "explicit-system.nix, via option systems.explicit.exports";
  };
assert
  configured.nixosConfigurations.consumer.config.captured.system.targetHost == "ssh.example.test";
assert configured.nixosConfigurations.consumer.config.captured.system.targetPort == 2222;
assert configured.nixosConfigurations.consumer.config.captured.system.skip;
assert configured.systemMetadata.consumer.targetHost == "ssh.example.test";
assert configured.systemMetadata.consumer.targetPort == 2222;
assert
  configured.systemMetadata.consumer.tags == [
    "custom"
    "x86_64"
  ];
assert configured.systemMetadata.consumer.skip;
assert fails valid.nixosConfigurations.producer.config.networking.hostName;
assert fails invalidType.exports.records;
assert fails invalidType.nixosConfigurations.broken.config.captured.globalExports.records;
assert fails unknownExport.exports;
assert fails invalidSystem.nixosConfigurations.broken.config.networking.hostName;
assert fails invalidSystem.nixosConfigurations.broken.config.system.build.toplevel.drvPath;
assert builtins.deepSeq invalidSystem.systemMetadata.broken true;
assert invalidSystem.systemMetadata.broken.tags == [ "x86_64" ];
assert invalidSystem.systemMetadata.broken.sopsFiles == [ ./exports.nix ];
assert fails
  (evaluate {
    broken.nixosModule.sops.secrets.example.sopsFile = 42;
  }).systemMetadata.broken;
assert lib.all
  (
    fields:
    let
      result = evaluate {
        consumer = {
          imports = [ systems.consumer ];
        }
        // fields;
      };
    in
    fails (builtins.intersectAttrs fields result.systems.consumer)
    && fails result.systemMetadata.consumer
  )
  [
    { domain = 42; }
    { domain = [ "invalid" ]; }
    { targetHost = 42; }
    { targetPort = "invalid"; }
    { targetPort = 0; }
    { targetPort = 65536; }
    { tags = "invalid"; }
    { tags = [ 42 ]; }
    { skip = "invalid"; }
  ];
assert (evaluate { empty.nixosModule = { }; }).exports.records == [ ];
assert priority.exports.records == [ "forced" ];
assert lib.all (system: system.config.captured.globalExports == priority.exports) (
  lib.attrValues priority.nixosConfigurations
);
true

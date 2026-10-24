{ lib, modulesPath, options, ... }:

with lib;

{
  imports = [
    (modulesPath + "/profiles/qemu-guest.nix")
  ];

  boot = {
    loader.systemd-boot.enable = true;
    kernelParams = [ "memhp_default_state=online" ];

    # Regenerate machine-id and ssh host keys from VM UUID
    initrd = {
      systemd = {
        # Removed options retain an untyped stub on newer nixpkgs revisions.
        enable = mkIf ((options.boot.initrd.systemd.enable.type.name or null) == "bool") true;

        services = {
          host-keys-setup = rec {
            requires = [ "initrd-root-fs.target" ];
            after = requires;
            wantedBy = [ "initrd.target" ];

            script = ''
              cat /etc/machine-id

              if [[ "$(cat /etc/machine-id)" != "$(cat /sysroot/etc/machine-id)" ]]; then
                echo "Machine id changed"
                rm /sysroot/etc/ssh/ssh_host_*
                mv /etc/machine-id /sysroot/etc/
              fi
            '';

            serviceConfig.Type = "oneshot";
          };
        };
      };
    };
  };

  networking = {
    useNetworkd = true;
  };

  services = {
    qemuGuest.enable = true;
  };

  fileSystems = {
    "/" = {
      label = "nixos";
      fsType = "ext4";
      autoResize = true;
    };
    "/boot" = {
      label = "boot";
      fsType = "vfat";
    };
  };

  boot.initrd.systemd.repart.enable = true;
  systemd.repart.partitions = {
    "10-root".Type = "linux-generic";
  };
}

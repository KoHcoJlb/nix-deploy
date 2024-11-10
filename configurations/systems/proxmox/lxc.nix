{ ... }:

{
  networking.domain = "proxmox";

  boot = {
    isContainer = true;
    loader.initScript.enable = true;
  };

  networking = {
    useDHCP = false;
    useHostResolvConf = false;
    useNetworkd = true;
  };
}

#!/usr/bin/env python

import json
import os
import subprocess
from argparse import ArgumentParser
from pathlib import Path
from typing import List

from ruamel import yaml

CACHE_DIR = Path("./cache")
SSH_CONFIG = os.getenv("NIX_DEPLOY_SSH_CONFIG")

if SSH_CONFIG:
  os.putenv("NIX_SSHOPTS", f"-F {SSH_CONFIG}")


def system_host(name: str) -> str:
  return subprocess.check_output(
    ["nix", "eval", "--raw", f"./flake#targetHost.{name}"],
    encoding="utf8"
  )


def system_names() -> List[str]:
  return json.loads(subprocess.check_output([
    "nix", "eval", "--json", "./flake#systemNames"
  ]))


def deploy_cmd(opts):
  for name in opts.name:
    host = system_host(name)
    ssh_host = f"root@{host}"
    action = "boot" if opts.reboot else "switch"

    subprocess.run([
      "nix", "build", f"./flake#toplevel.{name}", "-o", CACHE_DIR / name
    ], check=True)

    subprocess.run(
      [
        "nixos-rebuild", "--fast", action,
        "--flake", f"./flake#{name}",
        "--target-host", ssh_host
      ],
      check=True
    )

    if opts.reboot:
      config = ["-F", SSH_CONFIG] if SSH_CONFIG else []
      subprocess.run(["ssh", *config, ssh_host, "reboot"], check=True)


def build_cmd(opts):
  subprocess.run([
    "nix", "build",
    f"./flake#toplevel.{opts.name}"
  ], check=True)


def collect_keys_cmd(opts):
  MANAGER_KEY = yaml.scalarstring.PlainScalarString(
    "age1dhc4nkcnl9q8x0wj52q8qk9y2pqapup88ssc60e0g7y7u3wxxu0qjp88k8",
    "nixos-manager"
  )

  SOPS_CONF = ".sops.yaml"

  yml = yaml.YAML()

  hosts = system_names()

  rules = {}
  for host in hosts:
    res = subprocess.run(f"set -o pipefail; ssh-keyscan -t ed25519 -q {system_host(host)} | ssh-to-age",
                         shell=True, capture_output=True)
    if res.returncode == 0:
      key = res.stdout.decode().strip()
      rules[host] = {
        "host": host,
        "path_regex": fr"flake/hosts/{host}/.+\.sops\..+",
        "key_groups": [
          {
            "age": [
              MANAGER_KEY,
              key
            ]
          }
        ]
      }
    else:
      print(f"Failed to get key of {host}: {res.stderr.decode().strip()}")
      rules[host] = None

  try:
    with open(SOPS_CONF) as f:
      sops_conf = yml.load(f)
  except FileNotFoundError:
    sops_conf = {}

  sops_conf["keys"] = [MANAGER_KEY]

  creation_rules = sops_conf.setdefault("creation_rules", [])
  for idx, rule in enumerate(creation_rules):
    if host := rule.get("host"):
      if new_rule := rules.get(host):
        creation_rules[idx] = {**new_rule}
        new_rule["applied"] = True

  for rule in rules.values():
    if rule and not rule.get("applied"):
      creation_rules.append({**rule})

  creation_rules[:] = [r for r in creation_rules
                       if not r.get("host") or r.get("host") in rules]

  with open(SOPS_CONF, "w") as f:
    yml.dump(sops_conf, f)


if __name__ == "__main__":
  args = ArgumentParser()

  commands = args.add_subparsers()

  cmd = commands.add_parser("deploy")
  cmd.add_argument("-r", "--reboot", action="store_true")
  cmd.add_argument("name", nargs="+")
  cmd.set_defaults(handler=deploy_cmd)

  cmd = commands.add_parser("build")
  cmd.add_argument("name")
  cmd.set_defaults(handler=build_cmd)

  cmd = commands.add_parser("collect-keys")
  cmd.set_defaults(handler=collect_keys_cmd)

  opts = args.parse_args()

  opts.handler(opts)

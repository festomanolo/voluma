"""The `voluma` command: start the app from a terminal, find it, or copy it into Applications."""
import argparse
import os
import platform
import shutil
import subprocess
import sys

from . import APP_PATH, __version__

HELP = """\
usage: voluma [command]

  voluma             open VOLUMA
  voluma where       print where the bundled VOLUMA.app is
  voluma install     copy VOLUMA.app to /Applications (or ~/Applications with --user)
  voluma --version   print the version
"""


def _require_mac() -> None:
    if sys.platform != "darwin":
        sys.exit("voluma: VOLUMA is a macOS app (macOS 14 or later).")
    mac = platform.mac_ver()[0].split(".")
    if mac and mac[0].isdigit() and int(mac[0]) < 14:
        sys.exit(f"voluma: needs macOS 14 or later (this is {platform.mac_ver()[0]}).")
    if not os.path.isdir(APP_PATH):
        sys.exit("voluma: the app bundle is missing from this install. Reinstall with: pip install --force-reinstall nolo-voluma")


def _launch() -> int:
    _require_mac()
    # `open` hands the app to LaunchServices, so the terminal is free again immediately.
    return subprocess.call(["open", APP_PATH])


def _install(user: bool) -> int:
    _require_mac()
    dest_dir = os.path.expanduser("~/Applications") if user else "/Applications"
    os.makedirs(dest_dir, exist_ok=True)
    dest = os.path.join(dest_dir, "VOLUMA.app")
    if os.path.exists(dest):
        shutil.rmtree(dest)
    # ditto keeps the code signature, permissions and extended attributes intact; shutil.copytree can lose them.
    rc = subprocess.call(["ditto", APP_PATH, dest])
    if rc == 0:
        print(f"Installed {dest}")
    return rc


def main(argv=None) -> int:
    p = argparse.ArgumentParser(prog="voluma", add_help=False)
    p.add_argument("command", nargs="?", choices=["where", "install"])
    p.add_argument("--user", action="store_true")
    p.add_argument("-V", "--version", action="store_true")
    p.add_argument("-h", "--help", action="store_true")
    a = p.parse_args(argv)
    if a.help:
        print(HELP, end="")
        return 0
    if a.version:
        print(f"voluma {__version__}")
        return 0
    if a.command == "where":
        print(APP_PATH)
        return 0
    if a.command == "install":
        return _install(a.user)
    return _launch()


if __name__ == "__main__":
    sys.exit(main())

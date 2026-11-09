"""Platform wheel: the package carries the signed universal VOLUMA.app, so it is tagged for macOS only."""
import os
import sys
from setuptools import setup
from wheel.bdist_wheel import bdist_wheel

HERE = os.path.dirname(os.path.abspath(__file__))
APP = os.path.join(HERE, "src", "nolo_voluma", "VOLUMA.app", "Contents", "MacOS", "Voluma")


class MacWheel(bdist_wheel):
    def finalize_options(self):
        super().finalize_options()
        self.root_is_pure = False          # contains a compiled app, not just Python

    def get_tag(self):
        return "py3", "none", "macosx_14_0_universal2"


# Fail loudly instead of installing something that cannot run: this is a macOS app.
if sys.platform != "darwin" and any(a in sys.argv for a in ("bdist_wheel", "install", "develop", "build")):
    sys.exit("nolo-voluma is a macOS app (macOS 14 or later). It cannot be installed on this platform.")
if not os.path.exists(APP):
    sys.exit("VOLUMA.app is missing from src/nolo_voluma. Run pypi/build.sh, which copies it in from target/dist.")

setup(cmdclass={"bdist_wheel": MacWheel})

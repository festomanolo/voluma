"""VOLUMA for macOS, installable with pip. The app itself lives in this package as VOLUMA.app."""
import os

__version__ = "0.1.0"
APP_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "VOLUMA.app")

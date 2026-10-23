#!/usr/bin/env python3
"""Public-release platform gate. Exit 0 only with current native evidence for all required OSes."""
from pathlib import Path
import runpy

runpy.run_path(str(Path(__file__).resolve().parents[1] / "tests/platform_release/gate.py"), run_name="__main__")

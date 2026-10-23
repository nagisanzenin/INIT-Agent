#!/usr/bin/env python3
# s1fx: every template's gen/run (a copy of this file): `gen/run <seed> <outdir>`, as FORMAT.md's contract.
import os, sys
sys.dont_write_bytecode = True
here = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(here, "..", "..", "..", "lib"))
import fxgen
fxgen.generate(os.path.dirname(here), sys.argv[1], sys.argv[2])

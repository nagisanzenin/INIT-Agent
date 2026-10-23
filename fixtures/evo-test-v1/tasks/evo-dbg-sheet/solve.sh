#!/bin/sh
# reference: the workspace with every planted bug fixed (fix.py reads bugs.json)
exec /usr/bin/python3 -I -S -B "$(dirname "$0")/fix.py" all .

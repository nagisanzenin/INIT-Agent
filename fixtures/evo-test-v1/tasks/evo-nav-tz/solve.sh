# evo-nav-tz reference: re-render the package with CHANGE.md applied (cwd = a copy of the built workspace)
exec /usr/bin/python3 -I -S -B "$(dirname "$0")/gen_ws.py" . after

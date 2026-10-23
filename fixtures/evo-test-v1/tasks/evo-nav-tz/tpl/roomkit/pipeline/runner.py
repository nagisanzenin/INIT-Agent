"""run_pipeline(config): input -> steps -> output or report (README.md, "Pipelines")."""
from roomkit.errors import ConfigError, ParseError
from roomkit.io.drafts import to_booking
from roomkit.pipeline.sinks import write_output, write_report
from roomkit.plugins.config import check_pipeline, load_config
from roomkit.plugins.registry import get_importer, get_step
from roomkit.storage.factory import open_store


def read_file(path):
    try:
        with open(path, "r", encoding="utf-8", newline="") as f:
            return f.read()
    except OSError as e:
        raise ParseError("%s: %s" % (path, e.strerror or e))


def read_input(spec, rooms_cal):
    """(bookings, rooms calendar) for the input spec. File input gets ids 1, 2, ... in file order."""
    if "store" in spec:
        cal = open_store(spec["store"]).load()
        return cal.bookings_sorted(), rooms_cal if rooms_cal is not None else cal
    entry = get_importer(spec["format"])
    drafts = entry.fn(read_file(spec["path"]))
    out = []
    for i, d in enumerate(drafts, 1):
#@B
        out.append(to_booking(d, booking_id=i))
#@A
        zone = rooms_cal.zone_of(d.room) if rooms_cal is not None else "UTC"
        out.append(to_booking(d, booking_id=i, tz=zone))
#@E
    return out, rooms_cal


def build_steps(specs):
    steps = []
    for i, spec in enumerate(specs, 1):
        name = spec.get("step")
        try:
            steps.append(get_step(name).fn(spec))
        except ConfigError as e:
            raise ConfigError(e.message).at("step %d (%s)" % (i, name))
    return steps


def run_pipeline(config):
    """Run a pipeline config (a dict); returns {"read", "written", "to"}."""
    check_pipeline(config)
    rooms_cal = open_store(config["rooms"]).load() if config.get("rooms") else None
    steps = build_steps(config.get("steps", []))
    bookings, rooms_cal = read_input(config["input"], rooms_cal)
    read = len(bookings)
    for step in steps:
        bookings = step(bookings)
    if "output" in config:
        written = write_output(config["output"], bookings)
        to = config["output"]["path"]
    else:
        written = write_report(config["report"], bookings, rooms_cal)
        to = config["report"]["path"]
    return {"read": read, "written": written, "to": to}


def run_pipeline_file(path):
    return run_pipeline(load_config(path))

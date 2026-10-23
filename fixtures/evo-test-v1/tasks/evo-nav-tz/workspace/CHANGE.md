# Change: time zones with fixed UTC offsets

Today every time in roomkit is a wall-clock time without a zone. After this change rooms have a home zone and
everything that holds a booking's times also holds the zone those times are written in. The rules below are
global: they apply to every part of the package they describe, whether or not that part is named here. Behaviour
they do not change stays as README.md describes it.

## 1. Zones

A zone is written as a code from this table or as a literal offset. There is no daylight saving time.

| code | offset | code | offset | code | offset | code | offset |
|------|--------|------|--------|------|--------|------|--------|
| UTC  | +00:00 | IST  | +05:30 | ACST | +09:30 | EST  | -05:00 |
| CET  | +01:00 | NPT  | +05:45 | AEST | +10:00 | MST  | -07:00 |
| EET  | +02:00 | ICT  | +07:00 | NZST | +12:00 | PST  | -08:00 |
| MSK  | +03:00 | JST  | +09:00 | BRT  | -03:00 | HST  | -10:00 |

- Codes match case-insensitively and are normalised to upper case (`jst` is `JST`).
- A literal offset is `+HH:MM` or `-HH:MM` with MM one of 00, 15, 30, 45 and a value from -12:00 to +14:00
  inclusive. It is kept as written: `+05:30` stays `+05:30`, `+00:00` stays `+00:00` (it does not become `UTC`).
- Anything else is refused with `unknown zone 'TEXT'` (TEXT exactly as given), raised as
  `roomkit.errors.ZoneError`, a subclass of `ParseError`.

The canonical text (`JST`, `+05:30`) is the zone's *label*; labels are what objects, files and outputs carry.
Add `roomkit/util/zones.py` with at least:

    parse_zone(text) -> label            ("jst" -> "JST", "+05:30" -> "+05:30", else ZoneError)
    offset_minutes(label) -> int         ("IST" -> 330, "-03:00" -> -180)
    fmt_offset(minutes) -> str           (330 -> "+05:30", -480 -> "-08:00", 0 -> "+00:00")
    to_utc(dt, label), from_utc(dt, label), convert(dt, src_label, dst_label)   (naive datetimes)

Absolute (UTC) time = local time - offset: `2026-03-02 09:00` in `+05:30` is `2026-03-02 03:30` UTC.

## 2. Who carries a zone

- `Room.tz` is the room's home zone label, default `"UTC"`.
- Every object that holds a booking's start and end (a booking, and every object that holds or copies a booking's
  times) holds in `tz` the label of the zone they are written in, default `"UTC"`. `Booking.utc_start()` and
  `Booking.utc_end()` return the absolute times (naive datetimes). Importers' drafts (`roomkit.io.drafts.Draft`) hold
  `tz` too: the label, or None when the input gave no zone.
- Something created without a zone takes a default: a room `UTC`; a booking (or anything else that will hold a
  booking's times) the zone of its room, `UTC` when its room is not known (e.g. a pipeline reading a file without a
  rooms store). An object that copies a booking's times copies its zone.

## 3. Inputs

Every input through which a room or a booking's times enter the package accepts a zone: on the command line the
option `--tz ZONE` (on every command that creates or changes a room, or that books, waitlists or moves a booking's
times), in request bodies the key `tz`, in tabular text (CSV files, feeds, store files) the column `tz` - or the
column the input format already reserves for a zone -, in JSON input objects the key `tz`, in the block format the
field `TZ`. An empty value means no zone. A zone that is not valid is reported like any other bad value of that
input, at its usual place and with its usual outcome: `unknown zone 'XYZ'` with the input's place prefix
(`line N: `, `item N: `, `rooms.csv line 3: `); so a bad `--tz` is a usage error (exit 2), a bad zone in an import
file fails the import (exit 1), a bad `tz` in a request body or query is a 400.

An input with a zone notation of its own (the partner feeds of docs/FEEDS.md) is read in that notation: a compact
offset `+0530` and an offset in minutes east of UTC `330` are the literal offset `+05:30` (`+0000` and `0` are
`+00:00`), a zone written after the start time is read like any zone; a value that does not give a valid zone is
refused quoting the value as written (`unknown zone '+0520'`, `unknown zone '320'`, `unknown zone 'Mars'`).

Times given together with a zone are read in that zone. Moving a booking: with a zone, the new start is read in that
zone and the booking takes it; without, the new start is read in the booking's current zone, which it keeps (also
when it moves to a room in another zone). Changing a room's zone does not change its bookings' times or zones.

The block format also accepts an inline offset right after the time, `START:2026-03-02 09:00+05:30` (literal
offsets only, no codes). An inline offset on START or END applies to the whole booking. Checked in this order,
before the field rules:

1. each inline offset must be a valid literal offset: `line N: unknown zone '+14:30'` (N = its line);
2. if START and END both carry one they must be equal: `line N: END offset +09:00 does not match START offset
   +05:30` (N = the END line);
3. if a TZ field and an inline offset are both present their offsets must be equal: `line N: TZ JST (+09:00) does
   not match inline offset +05:30` (N = the TZ line; the TZ field's label, its offset written with `fmt_offset`,
   then the inline offset as written).

The booking's zone is then the TZ field's label if present, else the inline offset.

## 4. Records

A *record* is the full representation of one room, or of one object that holds a booking's times: store rows and
documents, the output of every exporter except `agenda` (`csv`, `json`, `block` and the calendar exporters of
docs/CALENDARS.md), the command line's `--json` objects and text tables of such objects, objects in responses
(old-style ones included). A record keeps the booking's own local times unchanged and adds the zone label as a field
named exactly `tz` after all its other fields: in a JSON object as the key `"tz"` (last), in CSV and other delimited
text and in text tables as a column `tz` (last), in key-value text as a `tz` line written with that format's
separator (the record's last line), in the block format as a `TZ:` line right after the `END:` line.

## 5. Views

Every other output that shows dates or times derived from bookings is a *view* (reports of every kind, messages,
summaries, the `agenda` exporter, free slots, ...). A view has a display zone, given like an input zone (`--tz` on every
command that shows a view, the query parameter `tz` on every request that shows one, the option `tz` of reports and
of the `agenda` exporter - and so `report --tz`, `export --opt tz=`, `GET /reports/NAME?tz=`, pipeline sink
options -), default `UTC`. In a view every date and time is that of the display zone: a booking belongs to the day
its start falls on there (date options and date ranges are dates there), hours of day are hours there, times are
printed there (`HH:MM+1` where an end falls on a later day than the start or day it is shown with), and earliest,
latest and "sorted by start" compare absolute times (ties broken as before).

Exceptions, exactly:

- `utilization` is per room and per opening day: a booking counts on the day it starts in its room's zone; open
  minutes come from the room's hours. It takes no `tz`.
- Free slots are computed for a day of the room's zone, from the room's hours, in the room's zone; the display zone
  (default: the room's zone) only converts the start times shown.
- `conflicts` and `owners` show no times and take no `tz`.

A report that takes no `tz` refuses it like any other option it does not take (`report 'utilization' does not take
option 'tz'`, exit 2 / status 400).

## 6. Comparisons and rules

- Every comparison between times of bookings, or between a booking and anything else that holds a booking's times
  (overlaps, conflicts, waitlist checks, earliest/latest, sorting by start), uses absolute time. `09:00-10:00 CET`
  and `08:30-09:30 UTC` overlap by 30 minutes; `11:00-12:00 CET` and `09:00-10:00 UTC` touch (both at 10:00 UTC) and
  do not overlap.
- Booking rules 2 (`booking must start and end on the same day`) and 4 (`outside opening hours ...`) are checked in
  the room's zone: convert start and end to the room's zone, then compare dates and clock times with the room's
  `opens`/`closes` (local times of the room). This holds wherever the rules are checked, including every re-check of
  existing bookings. Messages are unchanged. A booking 08:00-09:00 in `+05:30` for a room in `UTC` with hours
  08:00-18:00 is 02:30-03:30 in the room's zone: outside the opening hours.

## 7. Stores: schema version 3

Both stores now write schema version 3; saving always writes version 3, in every file. Every CSV file of a store
gains the column `tz` (last) and every JSON object of a room or of a booking-like entry gains `"tz"` (last); the
schema line is `#roomkit-schema: 3`.

Upgrading from version 1 or 2 (version 1 goes through version 2): a room gets `UTC`; anything holding a booking's
times gets the zone of its room as read from the store's rooms (`rooms.csv` of whatever version, or the document),
`UTC` when its room is not there. Each CSV file is still upgraded on its own: a version-3 `rooms.csv` next to older
other files (or the other way round) loads. A zone cell that is not a valid zone is a bad row
(`rooms.csv line 3: unknown zone 'XYZ'`). Versions above 3 are refused: `bookings.csv: unsupported schema version 4
(newest is 3)`, `plan.json: unsupported schema version 4 (newest is 3)`.

## 8. Pipelines

- `where` also takes the field `tz` (compares the label as text); `sort` by `start` follows rule 6.
- New step `to_zone` (`{"step": "to_zone", "tz": "JST"}`): converts each booking's start and end to that zone and
  sets its `tz` to the zone's label. A bad zone is a ConfigError `step N (to_zone): unknown zone 'XYZ'`.

# roomkit

Room bookings for a small office: rooms with opening hours, bookings, a waitlist, an audit history, importers and
exporters, validation rules, reports, a command line, a request handler and config-driven pipelines. Python 3.9,
standard library only. This file documents the current behaviour. CHANGE.md describes the change to make next.

    roomkit/errors.py   the error classes
    roomkit/util/       time parsing and formatting, text tables, CSV helpers, small text helpers
    roomkit/models/     Room, Booking, WaitEntry, HistoryEntry, Calendar (the aggregate), record conversion
    roomkit/storage/    the CSV-directory store, the JSON-file store, schema migration, open_store()
    roomkit/io/         booking importers and exporters, the rooms importer
    roomkit/validate/   field rules, booking rules, schedule-wide rules, rulesets
    roomkit/services/   book / cancel / move, series, rooms, free slots, the waitlist, merging imported bookings
    roomkit/io/feeds/   partner-feed importers, one per partner (docs/FEEDS.md)
    roomkit/io/calendars/  exporters for other calendar systems, one per system (docs/CALENDARS.md)
    roomkit/reports/    registry reports (agenda, utilization, ..., daily metrics), digest, stats
    roomkit/reports/grouped/  grouped reports, one per grouping (docs/GROUPED.md)
    roomkit/notify/     notification channels, one per channel (docs/NOTIFY.md)
    roomkit/cli/        the command line: roomkit.cli.main.main(argv) -> exit code
    roomkit/api/        a request handler: App(store).handle(Request(method, path, body=None)) -> Response
    roomkit/plugins/    the name registry (importers, exporters, reports, pipeline steps) and config checks
    roomkit/pipeline/   config-driven pipelines (input -> steps -> output or report)
    docs/               generated references of the four families above (part of this documentation)
    tests/              unittest suite: /usr/bin/python3 -m unittest discover -s tests

## Times

A datetime is written `YYYY-MM-DD HH:MM` (on input a `T` may replace the space; output always uses the space), a
date `YYYY-MM-DD`, a clock time `HH:MM` (00:00 to 23:59). Times are wall-clock times without a zone. Errors:
`bad datetime 'X' (want YYYY-MM-DD HH:MM)`, `bad date 'X' (want YYYY-MM-DD)`, `bad time 'X' (want HH:MM)`.
Where a time span is shown as text it is `09:00-10:00`, and `23:30-00:30+1` when the end is on a later day than the
start.

## Rooms and bookings

A room (`roomkit.models.room.Room`) has `id` (lowercase letters, digits and `-`), `name` (1-40 characters),
`capacity` (integer >= 1), `opens` and `closes` (`datetime.time`, default 08:00 and 18:00, opens before closes:
`opens must be before closes`) and `tags` (a list of strings, written `a;b` in CSV files and on the command line).

A booking (`roomkit.models.booking.Booking`) has `id` (int), `room` (a room id), `start` and `end` (naive
`datetime.datetime`), `title` (1-80 characters), `owner` (lowercase letters, digits and `-`) and `attendees`
(integer >= 0, default 1). New bookings get the id 1 + the highest id in the calendar (1 for an empty one).
`roomkit.models.calendar.Calendar` holds `rooms` (dict id -> Room), `bookings` (dict id -> Booking), `waitlist`
(dict id -> WaitEntry) and `history` (list of HistoryEntry).

Field rules (roomkit/validate/fields.py, used by importers, the command line and the request handler) give these
messages: `missing field 'F'`, `bad value 'X' for F (want a-z, 0-9, -)`, `bad integer 'X' for F`, `F must be >= N`,
`F longer than N characters`, plus the time errors above.

Booking rules, checked in this order whenever a booking is created or changed (book, series, move, import, waitlist
promotion) and when a room is edited (the first failure is reported):

1. `end must be after start`
2. `booking must start and end on the same day`
3. `booking longer than 8 hours`
4. `outside opening hours 08:00-18:00` (the room's hours): the start time must be >= opens and the end time <= closes
5. `5 attendees exceed capacity 4 of room 'atlas'`

Two bookings of the same room overlap when `a.start < b.end and b.start < a.end` (back to back is fine). Booking or
moving into an overlap is refused with `room 'atlas' is busy: conflicts with booking 3` (the lowest conflicting id).
Unknown ids: `unknown room 'x'`, `no booking 12`, `no waitlist entry 4`. A room id that exists already:
`room 'x' exists`.

Services: `roomkit.services.booking` `book(cal, room_id, start, end, title, owner, attendees=1)`,
`cancel(cal, booking_id)`, `move(cal, booking_id, start, room=None)` (keeps the duration);
`roomkit.services.series.book_series(cal, room_id, start, end, title, owner, count, every_days=7, attendees=1)` books
`count` (1-52) occurrences `every_days` apart, all or none (`occurrence 3: <reason>`);
`roomkit.services.rooms.edit_room(cal, room_id, changes)` changes name, capacity, opens, closes or tags and then checks
every booking of the room (lowest id first) against the booking rules with the changed room, refusing the change with
`booking 3 would break: <rule message>`; `roomkit.services.slots.find_free(cal, room_id, day, duration, step=30)`
returns the free start times on `day`: candidates run from the room's opening time in `step` minute steps while
start + duration <= closes; a candidate is free when [start, start + duration) overlaps no booking of the room.

## Waitlist

`roomkit.services.waitlist.join(cal, room_id, start, end, title, owner, attendees=1)` adds a waitlist entry (ids like
bookings: 1 + the highest entry id). It is accepted only when it passes the booking rules and its time overlaps a
booking of the room; a free time is refused with `room 'atlas' is free at that time: book it instead`. After every
cancel and every move (command line and request handler), `promote(cal)` walks the entries in id order and books each
entry whose time overlaps no booking of its room (bookings just made from earlier entries count) and that passes the
booking rules; the entry leaves the waitlist and the booking gets the next booking id. Command line: `cancel` and
`move` print one `booked 5 from waitlist entry 2` line per promotion after their own line; responses carry
`"promoted": [{"entry": 2, "booking": 5}]`.

## History

Every booking change appends a history entry `{seq, action, booking, room, start, end}`: `book` (book and each series
occurrence), `cancel` (the cancelled booking as it was), `move` (the booking after the move), `import` (each booking
added by an import) and `waitlist` (each promotion). `seq` is 1 + the highest seq. An operation that fails as a whole
(a series, an import with policy `fail`) leaves no entries.

## Validation rulesets

`roomkit.validate.ruleset.run_ruleset(cal, name)` checks a whole calendar. Rules: `fields` (each booking against the
field rules: `booking 3: title longer than 80 characters`), `booking` (each booking's first broken booking rule:
`booking 3: outside opening hours 08:00-18:00`, or `booking 3: unknown room 'x'`), `overlaps` (every overlapping pair
in a room: `bookings 3 and 7 overlap in room 'atlas' (30 min)`), `owner_overlap` (the same owner in two overlapping
bookings of different rooms: `owner 'ana' double-booked: bookings 2 and 5`). Rulesets: `default` = fields, booking,
overlaps; `strict` = default + owner_overlap; `lenient` = fields, overlaps. Issues come out rule by rule in that
order, pairs by (lower id, higher id), as `{"rule": ..., "bookings": [ids], "message": ...}`.

## Stores

`roomkit.storage.factory.open_store(path)`: a path ending in `.json` is a JSON-file store, anything else a
CSV-directory store. A missing file or directory loads as an empty calendar; `save(cal)` creates it. `load()`
returns a Calendar. Saving always writes the newest schema version and every file, rows sorted by id (history by
seq), `\n` line ends, standard CSV quoting (a cell holding a comma or a quote is quoted).

CSV directory: `rooms.csv`, `bookings.csv`, `waitlist.csv` and `history.csv` (a missing file is empty). Each starts
with the schema line `#roomkit-schema: 2`, then the header row, then one row per record:

    rooms.csv     id,name,capacity,opens,closes,tags
    bookings.csv  id,room,start,end,title,owner,attendees
    waitlist.csv  id,room,start,end,title,owner,attendees
    history.csv   seq,action,booking,room,start,end

A file without the schema line is version 1: rooms `id,name,capacity,hours` (hours `08:00-18:00`, no tags),
bookings and waitlist entries `id,room,date,from,to,title,owner` (no attendees: 1), history the same columns as
version 2. Each file is upgraded on its own when it is read (roomkit/storage/migrate.py).

JSON file: `{"schema": 2, "rooms": [...], "bookings": [...], "waitlist": [...], "history": [...]}` with rooms
`{"id", "name", "capacity" (int), "opens", "closes", "tags" (list)}`, bookings and waitlist entries
`{"id" (int), "room", "start", "end", "title", "owner", "attendees" (int)}` and history entries
`{"seq" (int), "action", "booking" (int), "room", "start", "end"}`, written with indent 2 and a final newline. A
document without `"schema"` is version 1 (`{"version": 1, ...}` with the version-1 room and booking fields, no
waitlist or history).

A file whose version is above the newest is refused: `rooms.csv: unsupported schema version 3 (newest is 2)` (the
name is the CSV file's name or the JSON file's base name). A bad row: `bookings.csv line 4: <message>` (the schema
line is line 1).

## Importers and exporters

Booking importers (registry names) turn text into drafts to merge; each problem names its place.

- `csv`: a header row and one row per booking. Required columns room, start, end, title, owner; optional attendees;
  other columns are ignored. Cells are stripped; empty lines are skipped. Errors `line N: ...` (the header is line
  1), a missing column `line 1: missing column 'owner'`.
- `json`: an array of objects with the keys room, start, end, title, owner and optional attendees (string or
  integer). Any other key: `item N: unknown key 'x'` (items count from 1).
- `jsonl` (registry only): one JSON object per non-empty line, the same keys; errors `line N: ...`.
- `block`: blocks of `KEY:VALUE` lines between `BEGIN:BOOKING` and `END:BOOKING`; keys ROOM, START, END, TITLE,
  OWNER, ATTENDEES and ID (ignored). Blank lines and lines starting with `#` are skipped. Errors:
  `line N: unknown field 'X'`, `line N: duplicate field 'X'`, `line N: expected BEGIN:BOOKING`,
  `line N: unterminated block` (N = the BEGIN line); a missing field is reported at the END:BOOKING line, a bad
  value at its own line.
- `partner` (registry only): our partner sites' feed, separated by `;`, with a header row. Required columns site,
  day (`DD.MM.YYYY`, else `line N: bad day 'X' (want DD.MM.YYYY)`), from, until, subject, contact; optional seats
  (attendees) and zone. A row books room `site` from `day from` to `day until`, titled `subject`, owned by `contact`.
  The zone column is accepted but not used yet (roomkit has no zones): times are taken as written.

The partner feeds are importers too: `feed_<partner>`, one per partner, each with its own format; docs/FEEDS.md
describes every one of them.

Importing (`roomkit.services.merge.merge_drafts(cal, drafts, policy)`) adds the bookings in file order. A booking
whose room is unknown, that breaks a booking rule or that overlaps an existing or earlier imported booking is
skipped (policy `skip`, reported as `{"where": "line 3", "reason": ...}`) or fails the whole import (policy `fail`:
`line 3: <reason>`, nothing added). The `where` of a csv/partner row or jsonl line is `line N`, of a json item
`item N`, of a block `line N` of its BEGIN line. A parse error always fails the whole import.

Rooms come in through `roomkit.io.rooms_import` (`import-rooms`, `POST /import-rooms`): CSV with required columns id,
name, capacity and the optional room columns; a bad value fails the whole import (`line N: <message>`), a room whose
id exists is skipped (`room 'x' exists`), result `{"added": [ids], "skipped": [...]}`.

Exporters write the bookings in the order they are given: `export` passes all bookings sorted by id, a pipeline
passes them in the order its steps leave them.

- `csv`: header `id,room,start,end,title,owner,attendees`.
- `json`: an array of objects with the keys id, room, start, end, title, owner, attendees (indent 2, final newline).
- `block`: per booking `BEGIN:BOOKING`, `ID:`, `ROOM:`, `START:`, `END:`, `TITLE:`, `OWNER:`, `ATTENDEES:`,
  `END:BOOKING`, a blank line between blocks, final newline.
- `agenda` (registry only): tab-separated (`\t`), header `date\ttime\troom\ttitle\towner`, one line per booking
  sorted by start, room, id; date = the start's date, time = the span (`09:00-10:00`). Option `room` keeps one room.
- `cal_<system>`: exporters for other calendar systems, one per system (docs/CALENDARS.md).

## Reports

Registry reports (`report NAME`, `GET /reports/NAME`, pipeline report sinks) return JSON-able rows; each report
module lists the options it takes (`OPTIONS`) and needs (`REQUIRED`); another option is refused with
`report 'conflicts' does not take option 'x'`, a missing one with `report 'agenda' needs option 'date'`.

- `agenda` (date, room): the bookings starting on `date`, rows `{"id", "room", "start": "09:00", "end": "10:00",
  "title", "owner"}` sorted by start, room, id. An end on a later day than the start is shown as `HH:MM+1`.
- `utilization` (date_from, date_to, room): per room sorted by id `{"room", "open_minutes", "booked_minutes",
  "percent"}`; open minutes = days in the range x (closes - opens), booked minutes = the minutes of the room's
  bookings that start on a day in the range; percent = booked x 100 / open rounded half up to one decimal, a string
  (`"37.5"`, `"0.0"` when nothing is open).
- `conflicts` (room): each overlapping pair `{"room", "a", "b", "overlap_minutes"}` with a < b, sorted by room, a, b.
- `busiest` (date_from, date_to, room): for each hour of day, the number of bookings touching it (a booking touches
  hour h when it overlaps [h:00, h+1:00); each booking counts at most once per hour of day). Only bookings starting
  in the date range (both ends optional, inclusive) count. Rows `{"hour": "09", "bookings": 3}` for hours with at
  least one booking, sorted by bookings descending then hour.
- `owners` (room): per owner `{"owner", "bookings", "minutes"}` sorted by minutes descending, then owner.
- `reminders` (date, before, room): for each booking starting on `date`, a reminder `before` minutes (default 15)
  before its start: `{"at": "YYYY-MM-DD HH:MM", "id", "owner", "title", "room", "starts": "HH:MM"}` sorted by start,
  then id.
- daily metrics (date_from, date_to, room; both dates required): one row `{"date", "value"}` per day of the range
  (empty days too), the bookings grouped by the day they start on: `daily_bookings` (count), `daily_minutes`,
  `daily_attendees` (sums), `daily_rooms`, `daily_owners` (distinct counts), `daily_longest` (minutes, 0 when none),
  `daily_average` (minutes rounded half up to one decimal, a string, `"0.0"` when none), `daily_first_start` (the
  earliest start `HH:MM`, null when none), `daily_last_end` (the latest end `HH:MM`, `HH:MM+1` when it is on a later
  day than the row's day, null when none). A range ending before it starts: `date_to is before date_from`.
- grouped reports `by_<name>` (date_from, date_to, room): bookings grouped by weekday, hour, week, part of day,
  month or date and room, one report per grouping and value (docs/GROUPED.md).

Notification channels (`roomkit.notify`, registry kind "notifier", options date (required) and room) write the
bookings of one day as a message for a chat or mail system, one module per channel (docs/NOTIFY.md).

Two views are not registry reports: the owner digest (`roomkit.reports.digest.digest(cal, owner, first, last)`): one
entry `{"date", "items": [{"id", "room", "time", "title"}]}` per day from `first` to `last` on which a booking of the
owner starts, sorted by date, items sorted by start then id, `time` the span; and the store summary
(`roomkit.reports.stats.stats(cal)`): `{"rooms", "bookings", "waitlist" (counts), "first_start" (the earliest start of
all bookings), "last_end" (the latest end), "busiest_room" (most bookings, ties: smallest id)}`, times as
`YYYY-MM-DD HH:MM`, null without bookings.

## Command line

`roomkit.cli.main.main(argv)` returns the exit code: 0 success, 1 a failed operation (unknown id, a booking rule, a
conflict, a store or import file error, `check` finding issues), 2 a usage error: bad arguments, any value given on
the command line that breaks its format or field rule (`bad datetime ...`, `missing field 'title'`), an unknown name
(`unknown report 'x'`, likewise `unknown importer`, `unknown exporter`, `unknown step`, `unknown ruleset`), a bad
report or exporter option, any pipeline ConfigError. Errors go to stderr as `error: <message>`. Global option
`--store PATH` (default `data`) before the command.

    rooms [--json]
    add-room ID --name N --capacity C [--opens HH:MM] [--closes HH:MM] [--tags a;b]
    edit-room ID [--name N] [--capacity C] [--opens HH:MM] [--closes HH:MM] [--tags a;b]
    list [--room R] [--json]
    book ROOM START END --title T --owner O [--attendees N] [--json]
    book-series ROOM START END --title T --owner O --count N [--every DAYS] [--attendees N]
    wait ROOM START END --title T --owner O [--attendees N] [--json]
    waitlist [--json]
    cancel ID
    move ID START [--room R]
    free ROOM DATE --duration MIN [--step MIN] [--json]
    agenda DATE [--room R] [--json]
    report NAME [--date D] [--from D] [--to D] [--room R] [--json]
    reminders DATE [--before MIN] [--room R] [--json]
    digest OWNER --from D --to D [--json]
    stats [--json]
    notify CHANNEL DATE [--room R]
    check [--ruleset NAME] [--json]
    history [--booking ID] [--json]
    import FORMAT FILE [--policy skip|fail] [--json]
    import-rooms FILE [--json]
    export FORMAT FILE [--opt KEY=VALUE]...
    pipeline CONFIG
    migrate

Text output: `booked 3`, `cancelled 3`, `moved 3` (then the promotions), `added room atlas`, `updated room atlas`,
`waitlisted 2`, `imported 2, skipped 1` (then `skipped line 3: <reason>` per skipped booking), `imported 2 rooms,
skipped 0`, `exported 4 bookings to out.csv` (all bookings of the store), `migrated data`, `pipeline: read 4, wrote
2 to out.json` (bookings read; bookings exported or report rows written), `book-series` prints `booked 4, 5, 6`,
free slots one `YYYY-MM-DD HH:MM` per line, reminders one `2026-03-02 08:45 ana: Standup in atlas at 09:00` line
each, digest the date then ` 09:00-10:00 atlas Standup` lines, notify the channel's message, stats `key: value`
lines, `check` one `rule: message` line per issue (`no issues`), `(none)` for an empty list, tables for the rest
(columns separated by two spaces, a dashes line under the header).

`--json` output: rooms `{"rooms": [room, ...]}`, list `{"bookings": [booking, ...]}`, book `{"booking": booking}`,
wait `{"entry": entry}`, waitlist `{"waitlist": [entry, ...]}`, history `{"history": [entry, ...]}`, free
`{"room", "date", "slots": [...]}`, agenda `{"date", "rows": [...]}`, report `{"report": name, "rows": [...]}`,
reminders `{"date", "reminders": [...]}`, digest `{"owner", "days": [...]}`, stats the summary, check
`{"ruleset", "issues": [...]}`, import `{"added": [ids], "skipped": [...]}`. A room is
`{"id", "name", "capacity", "opens", "closes", "tags"}`, a booking or waitlist entry
`{"id", "room", "start", "end", "title", "owner", "attendees"}`, a history entry
`{"seq", "action", "booking", "room", "start", "end"}`. `report` passes `--date`, `--from`, `--to`, `--room` as the
options date, date_from, date_to, room (only the ones given); `agenda` and `reminders` call those reports.

## Request handler

`roomkit.api.app.App(store).handle(Request(method, path, body=None))` returns a `Response` with `status` and a JSON
`body` text. The route table (roomkit/api/routes.py) maps a method and a path pattern to a handler named
`"module:function"`. Errors become `{"error": message}` with 404 (unknown id, no route), 405 (known path, other
method), 409 (conflict), 400 (bad input: parse, field, rule, option errors, unknown body keys or query parameters).

    GET    /rooms                       {"rooms": [...]}
    POST   /rooms                       body id, name, capacity, opens, closes, tags   201 {"room": ...}
    GET    /rooms/ID                    {"room": ...}
    PATCH  /rooms/ID                    body name, capacity, opens, closes, tags (edit_room)   {"room": ...}
    GET    /rooms/ID/free?date=D&duration=MIN[&step=MIN]          {"room", "date", "slots": [...]}
    GET    /bookings[?room=R]           {"bookings": [...]}
    POST   /bookings                    body room, start, end, title, owner, attendees   201 {"booking": ...}
    GET    /bookings/ID                 {"booking": ...}
    PATCH  /bookings/ID                 body start, room (moves; keeps the duration)  {"booking": ..., "promoted"}
    DELETE /bookings/ID                 {"cancelled": ID, "promoted": [...]}
    POST   /series                      body as POST /bookings + count, every (default 7)   201 {"bookings": [...]}
    GET    /waitlist                    {"waitlist": [...]}
    POST   /waitlist                    body as POST /bookings   201 {"entry": ...}
    DELETE /waitlist/ID                 {"removed": ID}
    GET    /agenda?date=D[&room=R]      {"date", "rows": [...]}
    GET    /reports/NAME?OPTION=VALUE   {"report": name, "rows": [...]} (query parameters are the report options)
    GET    /owners/OWNER/digest?date_from=D&date_to=D               {"owner", "days": [...]}
    GET    /stats                       the summary
    GET    /notify/CHANNEL?date=D[&room=R]  {"channel": CHANNEL, "text": the message}
    GET    /history[?booking=ID]        {"history": [...]}
    POST   /import/FORMAT[?policy=P]    body = the file text   {"added": [...], "skipped": [...]}
    POST   /import-rooms                body = the CSV text    {"added": [...], "skipped": [...]}
    GET    /v1/bookings[?where=R]       {"items": [item, ...]}   (version-1 clients)
    GET    /v1/bookings/ID              {"item": item}

Rooms, bookings, waitlist and history entries have the same keys as in the command line's JSON output. A version-1
item is `{"ref" (id), "where" (room), "when" (start), "until" (end), "what" (title), "who" (owner)}`.

## Pipelines

`roomkit.pipeline.runner.run_pipeline(config)` (command `pipeline CONFIG`, a JSON file; relative paths are relative
to the current directory) reads bookings, applies steps in order and writes an export or a report:

    {"input": {"format": "csv", "path": "in.csv"}  or  {"store": "data"},
     "rooms": "data",                                   (optional: a store whose rooms are used)
     "steps": [{"step": "where", "field": "room", "op": "eq", "value": "atlas"}, ...],
     "output": {"format": "json", "path": "out.json", "options": {}}
       or "report": {"name": "agenda", "path": "rows.json", "options": {"date": "2026-03-02"}}}

Bookings read from a store come in id order; bookings read from a file get the ids 1, 2, ... in file order and are
not checked against rooms or rules. Steps (registry names): `where` (field, op, value; fields id, room, title,
owner, attendees, start, end; ops eq, ne, lt, le, gt, ge, contains; id and attendees compare as integers, start and
end as their `YYYY-MM-DD HH:MM` text, the rest as text), `sort` (by: id, room, owner or start; `"desc": true` sorts
descending; ties keep id order either way), `limit` (n), `shift` (minutes: moves every booking), `rename_room`
(from, to). A report sink writes `{"report": name, "rows": [...]}` and sees the rooms of `rooms` (or of the input
store). Config errors (`unknown key 'x'`, `step 2 (where): unknown field 'x'`, unknown names, bad options) are
ConfigError.

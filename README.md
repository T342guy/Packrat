# Packrat

A small, self-hosted inventory for a garage, shed, basement or storage unit —
for everyone whose storage has quietly outgrown their memory of it.
One Rust binary, one SQLite file, no cloud account and no internet connection
required.

The point of it: **know what you own, find it in seconds, and see inside a box
without opening it.** Every box gets a printed label with a QR code and a
barcode — scan it with a phone or a barcode scanner and its contents open on
screen.

## Quick start

Three ways in, in increasing order of permanence.

**Try it:**

```bash
cargo run --release -- --seed-example    # drop --seed-example for an empty inventory
```

**Run it in a container:**

```bash
docker run -d --name packrat --restart unless-stopped \
  -p 8080:8080 -v packrat-data:/data \
  -e PACKRAT_PUBLIC_URL=http://192.168.1.24:8080 \
  ghcr.io/t342guy/packrat:latest
```

Images are built for x86-64. On a Pi or another arm64 box, take the
`aarch64` archive from the [releases](https://github.com/T342guy/packrat/releases)
instead, or build the image yourself with `docker build -t packrat .`.
`PACKRAT_PUBLIC_URL` matters here: QR codes encode an absolute address, and
inside a container Packrat can only see its own container-network address, which
no phone can reach — so tell it the host's address on your LAN. It says so on
startup if you forget. There's a `docker-compose.yml` in the repo if you'd
rather keep the settings in a file.

**Install it properly, starting at boot:**

```bash
sudo scripts/install.sh                  # system service, survives reboots
scripts/install.sh --user                # just for you, no root needed
scripts/install.sh --dry-run             # print the service file, change nothing
sudo scripts/install.sh --uninstall      # keeps your database
```

On Linux that writes a systemd unit (a sandboxed system service running as a
dedicated `packrat` account, or a user service with lingering enabled so it
starts without anyone logging in). On macOS it writes a LaunchDaemon under sudo,
or a LaunchAgent without. It builds from source if you have Rust, uses a binary
you already have if you don't, and points you at the container image if neither
applies. Uninstalling never touches the database.

Then open <http://localhost:8080>, or the `http://<your-ip>:8080` address the
server prints, from a phone on the same network.

```
  Packrat
  ───────
  database   /home/you/inventory.db
  local      http://localhost:8080
  network    http://192.168.1.24:8080   ← open this on your phone
  QR links   http://192.168.1.24:8080
```

### Options

Every flag can also be set by environment variable — usually easier in a
container. Flags win over the environment.

| Flag | Environment | Default | Meaning |
| --- | --- | --- |
| `-p, --port <PORT>` | `PACKRAT_PORT` | `8080` | Port to listen on |
| `--host <ADDR>` | `PACKRAT_HOST` | `0.0.0.0` | Bind address — the default makes it reachable on your LAN |
| `-d, --db <PATH>` | `PACKRAT_DB` | `./inventory.db` | Where the SQLite database lives |
| `--public-url <URL>` | `PACKRAT_PUBLIC_URL` | auto-detected LAN address | Base URL encoded into QR codes |
| `--seed-example` | `PACKRAT_SEED_EXAMPLE` | off | Fill an empty database with a small example garage |

`GET /api/health` returns `{"ok":true}` after touching the database, for
container health checks and uptime monitors.

For a build you can copy anywhere:

```bash
cargo build --release      # ./target/release/packrat, ~4 MB, no runtime deps
```

The frontend is compiled into the binary, so that one file plus your `.db` is
the whole system.

## How it works

### Containers nest

Everything that holds things is a *container*: an area, a shelf, a cabinet, a
drawer, a bin, a box, a bag. Containers live inside other containers, so
`Garage / North wall shelving / Camping gear` is just three rows pointing at
each other. Items live in exactly one container — or none, if you haven't
filed them yet.

Every container gets a short label code such as `BX-7K3Q`. The alphabet leaves
out `0/O/1/I/L/U`, so a code read off a taped-up label and typed into the
search box lands on the right box.

### Shelves show their boxes

Opening a shelf (or any container that holds other containers) lists every box
on it with its contents folded away underneath — expand a box to see and edit
what's in it without navigating away. Each row shows how long it's been since
that box was last verified.

### The scan-a-box workflow

1. Pack a box, add its items in the app.
2. **Print labels** → pick your label stock → tick the boxes → print. Each label
   carries a QR code, the code in text, and (where it fits) a list of what's
   inside.
3. Tape one label to each box and stack them.
4. Later: point a phone camera at the label. It opens
   `http://<server>/b/BX-7K3Q`, which redirects to that box's page — a full,
   current list of the contents, with photos.

### Barcode scanners

Packrat has a **Scanner** mode built for a keyboard-wedge barcode scanner — the
usual kind, wired or wireless, that types what it reads and presses Enter. Point
a browser on the garage machine at `#/scan`, and every scan becomes an action.
It works with a plain keyboard too, so you can try it before buying anything.

Four modes:

| Mode | A scan does this |
| --- | --- |
| **Look up** | Shows what the code is and where it lives — a box with its contents, or an item with its location. |
| **Put away** | Scan a box first to set the destination, then scan items to file them into it. Scanning something already in that box adds one to the count instead. |
| **Count +1** | Adds one to the item's quantity — for restocking. |
| **Take out −1** | Takes one away, for things being used up. |

Everything scanned is listed in a running session log, and each scan beeps
(different tones for found, moved and unknown) so you can work with your eyes on
the shelves. Scan a code Packrat has never seen and it offers to add it as a new
item — into the current destination box, with the barcode already filled in — or
to link it to something already listed.

Items and containers each have an optional **barcode** field, filled in by
scanning into it. Use a product's own UPC/EAN on an item, or, if a box already
wears a barcode sticker you'd rather not replace, put that on the container. A
barcode can only point at one thing, so a scan is never ambiguous.

**About scanner hardware:** cheap 1D laser scanners cannot read QR codes at all
— they only read linear barcodes. That's why labels can carry both. If you're
buying, a 2D imaging scanner reads both kinds and is worth the small extra cost;
if you already own a 1D laser, print labels with the barcode included and use
stock at least 48 mm wide.

### Label stock, including DYMO printers

**Print labels** is one page: pick the stock, pick the symbols, tick the boxes
you want, and the preview beside the list is the real renderer showing exactly
what will come out. Printing prints that preview. Opening it from a box's page
arrives with that box already ticked. DYMO (and other
label-printer) formats print **one label per page at the exact label size**,
which is how a LabelWriter expects to be driven from a browser.

| Format | Size | What fits on it |
| --- | --- | --- |
| A4/Letter sheet — with contents | tiled on plain paper | QR, code, name, location, up to 14 items |
| A4/Letter sheet — compact | tiled, 3 across | QR, code, name, location |
| DYMO 30332 | 1″ × 1″ square | QR, code, short name |
| DYMO 30336 | 1″ × 2⅛″ multipurpose | QR, code, name |
| DYMO 30334 | 2¼″ × 1¼″ multipurpose | QR, code, name, location, 4 items |
| DYMO 30252 | 1⅛″ × 3½″ address | QR, code, name, location, 5 items |
| DYMO 30323 | 2⅛″ × 4″ shipping | QR, code, name, location, 12 items |
| Custom size… | any width × height in mm | scaled automatically to the space |

Each label can carry a **QR code**, a **Code 128 barcode**, or both — the
`Symbols` picker.

Both symbols compete for the same millimetres, so the layout is worked out per
stock rather than fixed. On a label that is wide but short the barcode sits
beside the QR so the QR keeps its full height; otherwise it runs full width
underneath. The print page then reports what it actually produced — the QR's
size and millimetres per module, and the barcode's bar width — because a label
that is unreadable after you have cut it out is worth catching beforehand. A
phone wants roughly 0.40 mm per QR module; a 1D laser wants 0.33 mm bars.

The default, *Automatic*, will not buy a barcode at the price of an unreadable
QR: on 1″ and 1″ × 2⅛″ stock it prints the QR alone (0.43–0.57 mm per module),
and from 2¼″ upwards it prints both. Ask for both explicitly on small stock and
you'll get them, with a warning saying how tight they are.

`/labels` also answers directly, without the app around it, if you want to
bookmark a set or fetch one with `curl`: `?codes=`, `?all=1`, `?format=`,
`?symbols=`, `?tape=off`, and `?embed=1` for the bare labels the app embeds.

In the print dialog: choose the LabelWriter, set the label size to the matching
stock, margins to **none**, and scale to **100%** with "fit to page" off.
Anything else shrinks the codes and can push them off the label.

### Cutting labels off a paper sheet

Sheet labels print with **cut and tape margins**: an 8 mm strip top and bottom
marked *cut here · tape over this strip*, and 5 mm of clear space either side.
They do two jobs — a cut that wanders by a few millimetres takes margin instead
of taking the QR code, and packing tape has somewhere to land that isn't over a
code. Cut along the outer line. The strips can be turned off from the print page
if you'd rather fit more on a sheet.

They are drawn with rules and text rather than shading, because browsers drop
background graphics from printouts by default. Roll stock skips them: it's
peel-and-stick, and the space is too precious.

### Areas hold containers, not items

An area — a garage, a shed, a basement — is a place, and things belong in the
shelves and boxes inside it rather than loose in the room. Packrat enforces
that: an area's page offers to add a box or shelf rather than an item, areas
don't appear when choosing where an item lives, and the API refuses an item
assigned to one. Every container's page can add another container inside
itself, so building out a shelf's worth of boxes doesn't mean going back to
the overview each time.

On the 1″ square stock the QR is 16 mm across, which scans fine from a phone —
but the QR encodes the full URL, so a short base address (`http://192.168.1.24:8080`)
keeps the code sparse and easy to read. A long hostname makes it denser.

### Re-checking what's in a box

Inventories drift: things get borrowed, moved and used up. Every container
records when its contents were last confirmed, and one that holds items and
hasn't been verified within the re-check window (default **180 days**, set in
Settings) is flagged as needing a check.

**Check-ups** lists everything overdue, most stale first. From there:

- **Still fine** confirms a box without opening it.
- **Check** opens verification mode: a focused checklist of that box's
  contents. Tick things off as you find them, adjust quantities inline, edit
  anything that's wrong, mark a missing thing as **Gone**, add whatever turned
  up that wasn't listed — then **Mark as checked**, which resets the clock.

Empty containers are never flagged: there's nothing in them to verify.

### Renaming things

Anything's name can be changed at any time. Containers can be renamed from the
box page, from any shelf listing them, or from the **Rename** button on each row
of **Boxes & places** — where you can also change its kind, move it somewhere
else, edit its notes, or change its label code. Items are renamed from **Edit**
on any row. Tags are renamed or removed under **Settings → Tags**; renaming a
tag updates every item using it, and renaming onto an existing tag merges the
two.

Changing a container's *name* doesn't affect its label — the QR code encodes the
label code, not the name, so a renamed box keeps working with the label already
taped to it. Changing the *code* does mean reprinting.

The QR code encodes an absolute URL, so it must match how phones reach the
server. The address is auto-detected; if your machine's IP changes or you use a
hostname, set it once in **Settings → Address used in QR codes** (or pass
`--public-url`). Existing labels keep working as long as that URL stays valid —
the codes themselves never change.

### Searching

The search box at the top searches item names, notes, tags, container names and
label codes at once. Multiple words narrow the result (all must match), and
matches on an item's *name* rank above matches in its description. Searching
`camping` finds both the box called "Camping gear" and everything inside it.

### Photos

Photos are optional but useful: a photo of a box's contents makes it
recognisable at a glance. Images are shrunk in the browser before upload (max
1400 px, JPEG) so a 3 MB phone snapshot lands as ~250 KB, and they're stored in
the database — one file still holds everything.

## Backups

The database is a single SQLite file; copying it while the server is stopped is
a complete backup. From **Settings** you can also:

- **Export JSON** — readable, portable, no photo bytes.
- **Export JSON with photos** — everything, photos base64-encoded.
- **Export CSV** — items as a spreadsheet.
- **Restore from JSON** — replaces the current inventory, keeping ids so
  printed labels still resolve.

## HTTP API

Everything the UI does is a plain JSON endpoint, so scripts and scanners can
use it too.

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/bootstrap` | Containers, tags, stats and settings in one call |
| `GET` `POST` | `/api/containers` | List / create containers |
| `GET` `PUT` `DELETE` | `/api/containers/{id}` | Container with its ancestors, children and contents |
| `GET` | `/api/containers/{id}/qr.svg?size=240` | QR code for a container |
| `GET` | `/api/containers/{id}/barcode.svg` | Code 128 barcode for a container |
| `GET` | `/api/scan/{code}` | What a scanned code is: a container, an item, or unknown |
| `POST` | `/api/containers/{id}/verify` | Record that the contents were just confirmed |
| `GET` | `/api/stale` | Containers overdue for a check, most overdue first |
| `GET` | `/api/by-code/{code}` | Same detail, looked up by label code |
| `GET` `POST` | `/api/items` | Filter (`q`, `container`, `nested`, `tag`, `unfiled`, `sort`, `limit`) / create |
| `GET` `PUT` `DELETE` | `/api/items/{id}` | Fetch, replace, remove an item |
| `POST` | `/api/items/{id}/move` | `{"container_id": 3}` or `null` to unfile |
| `POST` | `/api/items/{id}/quantity` | `{"delta": -1}`, clamped at zero |
| `POST` | `/api/items/bulk-move` | Empty one box into another |
| `GET` | `/api/search?q=` | Items and containers, ranked |
| `GET` | `/api/tags`, `/api/stats` | Tag counts, inventory totals |
| `PUT` `DELETE` | `/api/tags/{name}` | Rename (merging on collision) or remove a tag |
| `GET` | `/api/label-formats` | Available label stock |
| `GET` `PUT` | `/api/settings` | QR base URL and the re-check window in days |
| `POST` | `/api/photos` | Multipart upload, field `file` |
| `GET` | `/photos/{id}` | Photo bytes |
| `GET` | `/api/export`, `/api/export.csv` | Backups |
| `POST` | `/api/import?confirm=replace` | Restore |
| `GET` | `/labels?codes=A,B&format=dymo-30332` | Printable labels (`symbols=auto\|qr\|both\|barcode`, `tape=on\|off`, `format=custom&w=&h=` in mm) |
| `GET` | `/b/{code}` | What a QR code resolves to |

Deleting a container never deletes belongings: its items become unfiled and
anything nested inside moves up one level.

## A note on security

There is no login. Anyone who can reach the port can read and change the
inventory, and the default bind address (`0.0.0.0`) exposes it to your whole
local network — which is what makes phone scanning work. Run it on a network
you trust, and don't port-forward it to the internet. To keep it to the machine
it runs on, use `--host 127.0.0.1` (QR scanning from a phone won't work then).

## Builds and releases

Every push, on any branch, runs formatting, clippy with warnings denied, the
unit tests, a release build, a smoke test that starts the binary and queries
it, and the benchmarks. Benchmark medians are published to the job summary as
a table; they do not gate the build, because runner timings are too noisy to
fail on, but a regression is visible while the change is still in progress.

**Releases** are one pipeline, run on every merge to `main` or on demand from
the Actions tab where you can choose whether to raise the patch, minor or major
part. In order, it:

1. works out the next version and bumps `Cargo.toml`, commits it back and tags it;
2. builds statically linked binaries for x86-64 and arm64 Linux from that tag,
   packaging each as both `.tar.gz` and `.zip` with the README, the licence and
   the installer alongside;
3. publishes a GitHub release listing every commit since the previous tag, with
   SHA-256 checksums;
4. builds the container image from the same commit and pushes it to
   `ghcr.io/t342guy/packrat`, tagged with that version, plus `latest` from
   `main`.

Keeping the image in the same run is the point: when it was a separate
workflow, it built whatever `Cargo.toml` said at its own ref, so an image from
`main` carried the version from *before* the bump. Now the release archives,
the git tag and the image all name the same version, and the build asserts that
the binary agrees before anything is published.

The image is x86-64 only because building arm64 images means emulation, which
cost twenty-five minutes a push against three. Release archives still carry an
arm64 binary for anyone running this on a Pi, since those build natively and
cost nothing.

Versions look like `0.1.1-2026.aug.21` — the semver part says what changed, the
date says when it was built. The day is deliberately not zero-padded: semver
forbids leading zeros in numeric pre-release identifiers, and cargo rejects
`0.1.1-2026.aug.05` outright. `scripts/next-version.sh` computes it and can be
run by hand to see what the next release would be called.

Pushing to any other branch runs the same build and packaging without bumping,
tagging or publishing anything, so the pipeline can be exercised safely.

## Development

```bash
cargo test     # search ranking, nesting, staleness, tags, scanning, Code 128
cargo bench    # timings over synthetic inventories of 1k, 4k and 16k items
cargo run      # debug server on :8080
```

The benchmarks exist because a real regression got through once: opening a
shelf re-queried every box on it separately, which was invisible on the example
data and 16x slower on a full one. That case is now measured directly, along
with search, scanning and listing. On a 4,000-item inventory — far larger than
a full garage — a barcode scan resolves in about 0.14 ms and opening a box
takes about 2 ms. Search is a `LIKE` scan and grows with the inventory: 4 ms at
1,000 items, 90 ms at 16,000. If anyone ever fills a garage that far, that is
the number to attack, probably with SQLite's FTS5.

Layout:

```
src/main.rs     CLI, routes, startup
src/db.rs       connection pool, schema migrations
src/store.rs    all SQL and the data-model rules
src/api.rs      JSON handlers
src/media.rs    photos, QR codes, printable labels
src/barcode.rs  Code 128 encoding
src/backup.rs   export and import
static/         frontend, embedded into the binary at compile time
Dockerfile      two-stage build; the runtime image is Alpine plus the binary
scripts/        install.sh — installs the binary and registers it to autostart
```

## License

Copyright © 2026 T342guy.

Packrat is free software: you can redistribute it and modify it under the terms
of the GNU General Public License, version 3, as published by the Free Software
Foundation. It is distributed in the hope that it will be useful, but WITHOUT
ANY WARRANTY — without even the implied warranty of MERCHANTABILITY or FITNESS
FOR A PARTICULAR PURPOSE. See [LICENSE](LICENSE) for the full text.

A running instance serves the licence at `/license` and shows the same notice
in its footer, so an offline machine can still show a user their rights. Every
release archive carries `LICENSE` alongside the binary.

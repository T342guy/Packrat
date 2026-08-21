# Garage Inventory

A small, self-hosted inventory for a garage, shed, basement or storage unit.
One Rust binary, one SQLite file, no cloud account and no internet connection
required.

The point of it: **know what you own, find it in seconds, and see inside a box
without opening it.** Every box gets a printed label with a QR code — scan it
with any phone on your network and its contents open on screen.

## Quick start

```bash
cargo run --release -- --seed-example    # drop --seed-example for an empty inventory
```

Then open <http://localhost:8080>, or the `http://<your-ip>:8080` address the
server prints, from a phone on the same network.

```
  Garage Inventory
  ────────────────
  database   /home/you/inventory.db
  local      http://localhost:8080
  network    http://192.168.1.24:8080   ← open this on your phone
  QR links   http://192.168.1.24:8080
```

### Options

| Flag | Default | Meaning |
| --- | --- | --- |
| `-p, --port <PORT>` | `8080` | Port to listen on |
| `--host <ADDR>` | `0.0.0.0` | Bind address — the default makes it reachable on your LAN |
| `-d, --db <PATH>` | `./inventory.db` | Where the SQLite database lives |
| `--public-url <URL>` | auto-detected LAN address | Base URL encoded into QR codes |
| `--seed-example` | off | Fill an empty database with a small example garage |

For a build you can copy anywhere:

```bash
cargo build --release      # ./target/release/garage-inventory, ~6 MB, no runtime deps
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

### Label stock, including DYMO printers

The print page lays labels out for the stock you choose. DYMO (and other
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

In the print dialog: choose the LabelWriter, set the label size to the matching
stock, margins to **none**, and scale to **100%** with "fit to page" off.
Anything else shrinks the QR code and can push it off the label.

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
| `GET` | `/labels?codes=A,B&format=dymo-30332` | Printable labels (`format=custom&w=&h=` in mm) |
| `GET` | `/b/{code}` | What a QR code resolves to |

Deleting a container never deletes belongings: its items become unfiled and
anything nested inside moves up one level.

## A note on security

There is no login. Anyone who can reach the port can read and change the
inventory, and the default bind address (`0.0.0.0`) exposes it to your whole
local network — which is what makes phone scanning work. Run it on a network
you trust, and don't port-forward it to the internet. To keep it to the machine
it runs on, use `--host 127.0.0.1` (QR scanning from a phone won't work then).

## Development

```bash
cargo test     # search ranking, nesting, staleness, tags and label codes
cargo run      # debug server on :8080
```

Layout:

```
src/main.rs     CLI, routes, startup
src/db.rs       connection pool, schema migrations
src/store.rs    all SQL and the data-model rules
src/api.rs      JSON handlers
src/media.rs    photos, QR codes, printable labels
src/backup.rs   export and import
static/         frontend, embedded into the binary at compile time
```

# OrbitScope

English | [简体中文](README.md)

A near-real-time monitor for objects in low Earth orbit, written in Rust. It fetches orbital element sets from
public sources, propagates them locally with SGP4, filters by altitude range (100–1000 km by default) and minimum
size (≥ 10 cm by default), and shows the result on a 3D globe in the browser, on the command line and through an API.

- **Data sources**: CelesTrak (no account), Space-Track (free account; the full catalog including all debris),
  local TLE / OMM files. Disk cache, strict compliance with each source's rate rules, works offline.
- **Computation**: SGP4/SDP4 propagation; all 30,000 objects in about 8 ms (4 cores).
- **Filters**: altitude (orbit range or instantaneous), size (estimated from RCS), type, country, inclination,
  search by name or number.
- **History archive**: every element set is written once with the time it was fetched and never overwritten;
  capped raw snapshots are kept as well. Pick any moment and see what was known at that time, reproducibly.
  Three ways to collect (web server / standalone daemon / system timer).
- **Multilingual UI**: 简体中文 / English / 繁體中文（香港）; new strings are translated automatically at release time.
- **Views**: CesiumJS 3D globe (live point cloud, orbit tracks, statistics, time replay — each visitor picks their
  own time without affecting anyone else), command line (table / JSON / CSV), REST + WebSocket API.

## Quick start

```bash
cargo build --release                             # needs Rust 1.89+
./target/release/orbitscope serve                 # open http://127.0.0.1:8080/
```

Or with Docker (the image bundles CesiumJS, so the browser needs no CDN; amd64 and arm64):

```bash
docker run -d -p 127.0.0.1:8080:8080 -v orbitscope-data:/var/lib/orbitscope ghcr.io/70sdushuren/orbitscope:latest
# or build from source
docker compose up -d
```

Prebuilt binaries (Linux x86_64 / arm64, macOS Apple Silicon, Windows x86_64) are on the
[Releases](https://github.com/70sDushuren/orbitscope/releases) page. The Docker image and the Linux arm64 binary
are published for versions after v0.2.1.

Command-line examples:

```bash
orbitscope fetch                                  # fetch and cache data
orbitscope list --alt 400..450 --type debris --sort alt
orbitscope show 25544 --track 90m --step 5m
orbitscope export -f csv -o leo.csv
orbitscope stats --bin 100
orbitscope history status                         # archive disk usage
orbitscope --policy known-at list --at 2026-11-16T08:00:00Z   # reproduce the data as of a moment
```

To use Space-Track, set `[sources.spacetrack] enabled = true` in the config file and provide the credentials
through the environment variables `ORBITSCOPE_SPACETRACK_USER` / `ORBITSCOPE_SPACETRACK_PASSWORD`
(never put them in the config file).

Changing settings from the web page when the server runs in Docker, behind a reverse proxy, or on another
machine requires an API token: set `ORBITSCOPE_API_TOKEN` (at least 16 characters) for the service and enter the
same value in the "API token" field of the "Time & data" panel. Viewing, filtering and time replay need no token.

> **About size**: public catalogs do not contain physical dimensions. "Size" here is an equivalent diameter
> estimated from radar cross-section (RCS). The US Space Surveillance Network's catalog threshold in low Earth
> orbit is roughly 10 cm, so the public catalog is essentially the set of objects ≥ 10 cm.

## Documentation

The detailed documentation is currently in Chinese.

| Document | Contents |
|----------|----------|
| [User guide](docs/05-user-guide.md) | Installation, configuration, command line, web UI, API, deployment, FAQ |
| [Project plan](docs/01-project-plan.md) | Scope, milestones, risks |
| [Requirements](docs/02-requirements.md) | Functional and non-functional requirements |
| [Design](docs/03-design.md) | Architecture, data model, algorithms, API and WebSocket protocol, decision records |
| [References](docs/04-references.md) | Public data services and open-source projects |
| [Changelog](CHANGELOG.md) | What changed in each version |

## Project layout

```
crates/orbit-core     core computation (parsing, propagation, frames, size, filters); no IO
crates/orbit-sources  data sources, cache, rate limiting
crates/orbit-archive  history archive (SQLite element-set history, raw snapshots, time policies)
crates/orbit-server   REST + WebSocket server with the embedded frontend
crates/orbitscope     command-line program
web/                  browser frontend (plain ES modules)
```

## License

MIT or Apache-2.0, at your option. Data remains the property of its sources; use of Space-Track data is subject
to the Space-Track user agreement.

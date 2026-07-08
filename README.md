# runtab

Local-first token and cost ledger for AI coding agents (Claude Code, Codex,
and others). runtab scans agent logs on your machine, keeps a private SQLite
ledger, and gives you daily/model/project/session breakdowns plus a local
dashboard. Nothing leaves your machine unless you opt in to sync.

## Install

Run without installing (prebuilt binaries for Linux x64/arm64, macOS
arm64, Windows x64):

```
npx runtab daily
```

Or install globally:

```
npm install -g runtab
```

Or build from source with Cargo:

```
cargo build --release -p runtab
```

The binary is at `target/release/runtab`.

To include the local dashboard UI in the binary, build the UI first, then
build with the `embed-ui` feature:

```
cd ui && npm ci && npm run build && cd ..
cargo build --release -p runtab --features embed-ui
```

Without `embed-ui`, the CLI still works fully; only `runtab serve` (the local
dashboard) needs the embedded assets.

## Usage

```
runtab scan              # scan agent logs into the ledger (full backfill on first run)
runtab daily              # token/cost totals per day
runtab models              # token/cost totals per model
runtab projects            # token/cost totals per project
runtab sessions             # token/cost totals per session
runtab tools               # estimated context tokens by tool-call type
runtab serve                # run the local dashboard (embedded SPA + local JSON API)
```

Add `--json` to any reporting subcommand for machine-readable output. Use
`--db <path>` to point at a specific ledger database.

### Local dashboard

`runtab serve` starts a local web server (default port 7822, auto-increments
if busy, or set `RUNTAB_PORT`) that serves the dashboard UI and a local JSON
API reading from your ledger. It only binds locally and does not send any
data anywhere.

## Privacy

runtab is local-first by default. Logs are scanned and stored in a local
SQLite database on your machine. No data is sent anywhere unless you
explicitly enable sync.

## Sync (optional)

runtab can optionally sync your ledger to a server you control, so usage
data is available across machines. Sync is off by default.

```
runtab sync login    # authorize this machine in a browser and enable sync
runtab sync status   # show sync state, account, and known machines
runtab sync now       # push and pull once
runtab sync off        # disable sync on this machine (local data untouched)
runtab sync delete       # wipe the synced account on the server
runtab sync auto on       # install a scheduled background sync tick
runtab sync auto off        # remove the scheduled background sync tick
runtab sync auto status       # show whether auto-sync is installed
```

The sync server endpoint is configurable via the `RUNTAB_SERVER_URL`
environment variable. It defaults to the hosted service at
`https://api.runtab.ai`. To self-host, point it at any compatible sync
server you run yourself.

## License

MIT. See [LICENSE](LICENSE).

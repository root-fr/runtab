# runtab

Local-first token and cost ledger for AI coding agents (Claude Code, Codex,
and others). runtab scans agent logs on your machine, keeps a private SQLite
ledger, and gives you daily/model/project/session breakdowns plus a local
dashboard. Nothing leaves your machine unless you opt in to sync.

## Usage

```
npx runtab scan      # scan agent logs into the ledger (full backfill on first run)
npx runtab daily     # token/cost totals per day
npx runtab models    # token/cost totals per model
npx runtab projects  # token/cost totals per project
npx runtab sessions  # token/cost totals per session
npx runtab tools     # estimated context tokens by tool-call type
npx runtab serve     # run the local dashboard (embedded SPA + local JSON API)
```

Add `--json` to any reporting subcommand for machine-readable output.

This package installs a prebuilt binary via a platform-specific
optionalDependency (Linux x64/arm64, macOS x64/arm64, Windows x64). On other
platforms, build from source with Cargo.

Source, docs, and issues: <https://github.com/root-fr/runtab>

## License

MIT.

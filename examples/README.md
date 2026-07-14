# Examples

Small, self-contained scripts. Run everything from this directory with a
built `clipring` on your `PATH` (or use `cargo run --`). Both scripts use a
temporary state directory, so they never touch your real history.

## seed-demo.sh — a populated ring to play with

```bash
sh seed-demo.sh
```

Seeds four realistic entries (a connect string, a kubectl command, an SQL
query, a pinned deploy token placeholder) into a throwaway state dir, then
shows `list`, `search`, `paste`, and a `decode` round trip. Good for a
first look at the row format without copying anything real.

## shell-integration.sh — helpers for your rc file

```bash
cat shell-integration.sh   # then copy what you like into ~/.bashrc / ~/.zshrc
```

Defines three tiny helpers:

- `yy` — copy stdin (or arguments): `git rev-parse HEAD | yy`
- `pp` — paste entry N to stdout: `pp | psql`, `pp 2 > snippet.sql`
- `yl` — interactive picker bound to history: re-copy anything with two keys

Nothing is sourced automatically; the file is meant to be read and cherry-picked.

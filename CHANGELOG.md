# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-07-13

### Added

- OSC 52 emission: `clipring copy` sends the terminal-native clipboard sequence to `/dev/tty` (falling back to stdout when no terminal is attached), so copies work identically on local shells and across any number of SSH hops.
- Multiplexer passthrough: automatic tmux (`ESC Ptmux;` envelope with doubled ESCs) and GNU screen (≤ 768-byte DCS chunks) wrapping, detected from `$TMUX`/`$TERM` and overridable with `--wrap auto|none|tmux|screen`.
- History ring: every copy is recorded to `~/.local/state/clipring/history.jsonl` (newest first), with byte-identical deduplication that promotes instead of duplicating, stable ids across processes, and capacity-based eviction (default 50, `--capacity`/`CLIPRING_CAPACITY`).
- Pinning: `pin`/`unpin` protect entries from eviction and from plain `clear`; capacity applies to unpinned entries only.
- Retrieval: `paste [INDEX]` prints byte-identical content to stdout; `list [--json] [-n N]` renders control-safe previews; `search PATTERN` filters case-insensitively with grep-like exit codes; `pick [--print]` shows a numbered menu, re-copies the choice, and promotes it.
- `emit` (stdin → sequence without storing) and `decode` (extract and base64-decode every OSC 52 set from a byte stream, unwrapping tmux/screen envelopes) for scripting and debugging.
- Size-limit policy: payloads whose base64 exceeds the limit (default 100 000 bytes, `--limit`, `CLIPRING_LIMIT`) are stored but not emitted by `copy`, and are a hard error for `emit`/`pick`.
- Robust state handling: atomic temp-file + rename saves, damaged JSONL lines skipped with a warning instead of poisoning the ring, XDG state-dir resolution (`CLIPRING_STATE` override).
- Zero runtime dependencies: base64 codec, OSC 52 framing, flat-JSON parser, and picker are all implemented on the Rust standard library.
- Test suite: 64 unit tests, 28 CLI integration tests against the compiled binary, and `scripts/smoke.sh`.

[0.1.0]: https://github.com/JaydenCJ/clipring/releases/tag/v0.1.0

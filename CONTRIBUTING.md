# Contributing to clipring

Thanks for your interest in improving clipring. Issues, discussions and pull requests are all welcome.

## Getting started

Prerequisites: Rust 1.75 or newer (stable toolchain).

```bash
git clone https://github.com/JaydenCJ/clipring.git
cd clipring
cargo build
cargo test
bash scripts/smoke.sh
```

`scripts/smoke.sh` exercises the built binary end to end against a temporary state directory — copy → decode round trips through all three wrap modes, the ring across processes, pinning under capacity pressure, and damaged-state recovery. It finishes in well under a minute and must print `SMOKE OK`.

## Before you open a pull request

1. `cargo fmt` — formatting is enforced.
2. `cargo clippy --all-targets -- -D warnings` — clippy must be clean.
3. `cargo test` — unit tests and the CLI integration tests must pass.
4. `bash scripts/smoke.sh` — the smoke test must print `SMOKE OK`.
5. Add tests for behavior changes. The protocol and data logic lives in pure modules (`osc52`, `base64`, `ring`, `jsonl`, `textutil`, `picker`) that take all inputs as arguments — no clock, no environment, no I/O — and are easy to unit-test; please keep it that way.

## Ground rules

- Keep dependencies at zero. The base64 codec, OSC 52 framing and JSON parsing are implemented in-tree precisely so that `cargo install` never pulls anything; adding a dependency needs a very strong justification in the PR description.
- No network calls, ever. clipring's only outputs are the terminal byte stream and a local state file.
- Escape-sequence handling must stay symmetric: anything `emit` can produce, `decode` must extract. Add both directions to tests when touching `osc52.rs`.
- Previews must remain control-safe: clipboard content is untrusted and may contain escape sequences; nothing from an entry may reach the terminal unneutralized except through `paste`/`pick --print`.
- Code comments and doc comments are written in English.

## Reporting bugs

Please include the `clipring --version` output, your terminal emulator and multiplexer (with versions), the `clipring info` output, and — for emission bugs — the raw sequence captured with `clipring emit ... | xxd`. Wrap-detection bugs are much easier to fix with the values of `$TMUX` and `$TERM` at the time.

## Security

If you find a security issue (e.g. escape-sequence injection through previews or the decoder), please do not open a public issue. Use GitHub's private vulnerability reporting on this repository instead.

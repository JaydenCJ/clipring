#!/usr/bin/env bash
# Smoke test: builds clipring, then exercises the real end-to-end path —
# copy -> OSC 52 emission -> decode round trip (bare, tmux, screen), the
# history ring across processes (dedupe, pin, eviction), pick, search, and
# damaged-state recovery. Self-contained: temp state dir, no network, no tty.
set -euo pipefail

cd "$(dirname "$0")/.."

fail() { echo "SMOKE FAIL: $*" >&2; exit 1; }

echo "[smoke] building..."
cargo build --quiet
BIN=target/debug/clipring

WORK=$(mktemp -d "${TMPDIR:-/tmp}/clipring-smoke.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
STATE="$WORK/state"
export CLIPRING_STATE="$STATE"
unset CLIPRING_CAPACITY CLIPRING_LIMIT CLIPRING_WRAP TMUX 2>/dev/null || true
export TERM=xterm-256color

# --- 1. version/help sanity -------------------------------------------------
"$BIN" --version | grep -q '^clipring 0\.1\.0$' || fail "--version mismatch"
"$BIN" --help | grep -q 'COMMANDS:' || fail "--help missing sections"
echo "[smoke] version/help ok"

# --- 2. copy emits a decodable OSC 52 sequence ------------------------------
printf 'hello from clipring' | "$BIN" copy > "$WORK/seq.bin" 2> "$WORK/copy.err"
grep -q 'copied 19 B' "$WORK/copy.err" || fail "copy status line missing"
# No tty in this shell, so the sequence lands on stdout; check the frame.
head -c 7 "$WORK/seq.bin" | od -An -c | grep -q '033' || fail "no ESC in emitted sequence"
"$BIN" decode < "$WORK/seq.bin" > "$WORK/decoded.txt"
[ "$(cat "$WORK/decoded.txt")" = "hello from clipring" ] || fail "decode round trip"
echo "[smoke] copy -> decode round trip ok"

# --- 3. wrapped emission survives decode (tmux + screen) --------------------
for wrap in tmux screen; do
  printf 'wrapped-%s' "$wrap" | "$BIN" emit --wrap "$wrap" > "$WORK/wrap.bin"
  got=$("$BIN" decode < "$WORK/wrap.bin")
  [ "$got" = "wrapped-$wrap" ] || fail "$wrap wrap round trip (got: $got)"
done
grep -q $'\x1bPtmux;' "$WORK/wrap.bin" && fail "screen wrap must not use tmux envelope"
echo "[smoke] tmux/screen passthrough round trips ok"

# --- 4. the ring across processes: dedupe, order, paste ---------------------
printf 'second entry' | "$BIN" copy --no-emit 2>/dev/null
printf 'third entry'  | "$BIN" copy --no-emit 2>/dev/null
printf 'hello from clipring' | "$BIN" copy --no-emit 2>/dev/null  # dupe -> promote
[ "$("$BIN" list | wc -l)" -eq 3 ] || fail "dedupe failed: expected 3 entries"
"$BIN" list | head -1 | grep -q 'hello from clipring' || fail "promoted entry not newest"
[ "$("$BIN" paste 1)" = "third entry" ] || fail "paste 1 mismatch"
"$BIN" list --json | grep -q '"text":true' || fail "list --json missing text flag"
echo "[smoke] ring dedupe/order/paste ok"

# --- 5. binary content is preserved byte-for-byte ---------------------------
printf 'a\0b\xff\x1bc' > "$WORK/binary.in"
"$BIN" copy --no-emit < "$WORK/binary.in" 2>/dev/null
"$BIN" paste > "$WORK/binary.out"
cmp -s "$WORK/binary.in" "$WORK/binary.out" || fail "binary round trip not identical"
"$BIN" list | head -1 | grep -q '(binary:' || fail "binary preview missing"
echo "[smoke] binary round trip ok"

# --- 6. pin survives capacity pressure --------------------------------------
"$BIN" rm 0 2>/dev/null   # drop the binary entry
"$BIN" pin 2 2>/dev/null  # pin "second entry" (oldest)
for i in 1 2 3; do printf 'noise-%s' "$i" | "$BIN" --capacity 2 copy --no-emit 2>/dev/null; done
"$BIN" --capacity 2 list | grep -q '^\* .*second entry' || fail "pinned entry evicted"
"$BIN" --capacity 2 list | grep -q 'noise-1' && fail "capacity did not evict oldest unpinned"
echo "[smoke] pin + eviction ok"

# --- 7. search and pick ------------------------------------------------------
"$BIN" search NOISE | grep -q 'noise-3' || fail "case-insensitive search"
if "$BIN" search no-such-thing >/dev/null 2>&1; then fail "search no-match must exit 1"; fi
echo 0 | "$BIN" pick --print > "$WORK/picked.txt" 2>/dev/null
[ "$(cat "$WORK/picked.txt")" = "noise-3" ] || fail "pick --print mismatch"
echo "[smoke] search + pick ok"

# --- 8. oversize policy -------------------------------------------------------
head -c 200000 /dev/zero | tr '\0' 'x' | "$BIN" copy 2> "$WORK/big.err" > "$WORK/big.out"
grep -q 'exceeds limit' "$WORK/big.err" || fail "oversize warning missing"
[ -s "$WORK/big.out" ] && fail "oversize payload must not be emitted"
"$BIN" paste | wc -c | grep -q '^200000$' || fail "oversize entry must still be stored"
"$BIN" rm 0 2>/dev/null
echo "[smoke] oversize policy ok"

# --- 9. damaged state degrades gracefully ------------------------------------
echo '{torn' >> "$STATE/history.jsonl"
"$BIN" list 2> "$WORK/warn.err" >/dev/null
grep -q 'skipped 1 damaged' "$WORK/warn.err" || fail "damaged-line warning missing"
"$BIN" info | grep -q 'capacity: 50' || fail "info output"
echo "[smoke] damaged-state recovery ok"

echo "SMOKE OK"

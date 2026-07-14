# OSC 52, passthrough envelopes, and what clipring emits

This note documents the exact byte sequences clipring produces and consumes,
for terminal implementers and for anyone debugging a copy that "didn't take".

## The base sequence

OSC 52 asks the terminal emulator to set a selection:

```
ESC ] 5 2 ; Pc ; Pd BEL
```

- `Pc` — the selection parameter. clipring emits `c` (system clipboard,
  default) or `p` (X11 primary selection, `--primary`).
- `Pd` — the content, encoded as standard RFC 4648 base64 (padded).
- Terminator — clipring emits `BEL` (`0x07`) because it is the most widely
  accepted; the decoder additionally accepts `ST` (`ESC \`).

Because this travels the terminal byte stream, it works through any number of
SSH hops without agent forwarding, X forwarding, or a clipboard daemon on the
remote side. The *local* terminal does the copy.

A `Pd` of `?` is a *query* (asking the terminal to report the clipboard).
clipring never emits queries and `clipring decode` skips them.

## tmux passthrough

tmux consumes escape sequences it does not recognize. To reach the outer
terminal, the sequence is wrapped in a DCS passthrough envelope with every
inner `ESC` doubled:

```
ESC P t m u x ;  <sequence with ESC -> ESC ESC>  ESC \
```

tmux ≥ 3.3 requires `set -g allow-passthrough on` in `~/.tmux.conf` for this
envelope to be honored. (Alternatively, `set -g set-clipboard on` makes tmux
itself forward bare OSC 52 — in that case run clipring with `--wrap none`.)

## GNU screen passthrough

screen forwards DCS content verbatim but silently drops device control
strings beyond a small internal buffer. clipring therefore splits the whole
sequence into chunks of at most 768 bytes, each wrapped in its own DCS:

```
ESC P <chunk-1> ESC \  ESC P <chunk-2> ESC \  ...
```

The outer terminal reassembles the chunks into one OSC 52 sequence.

## Detection order

`--wrap auto` (the default) picks the envelope from the environment:

1. `$TMUX` set and non-empty → `tmux`
2. `$TERM` starts with `screen` → `screen`
3. otherwise → `none`

`$TMUX` is checked first because tmux sessions usually run with
`TERM=screen-256color`. Override with `--wrap` or `CLIPRING_WRAP` when
detection is wrong (e.g. tmux nested inside screen).

## Size limits

Terminals cap the accepted payload: tmux historically clamps around 100 KB
of base64, and several emulators drop oversized sequences entirely — often
silently. clipring refuses to emit payloads whose base64 exceeds the limit
(default 100 000 bytes, `--limit N`, `0` = unlimited) instead of letting the
copy vanish: `copy` still records the entry and says so, `emit` and `pick`
fail loudly.

## The decoder

`clipring decode` is the exact inverse of emission: it unwraps tmux and
screen envelopes (un-doubling ESCs, reassembling chunks), scans for
`ESC ] 5 2 ;`, splits the selection parameter from the payload, and
base64-decodes it — leniently with respect to interleaved whitespace, since
multiplexers may re-flow long sequences. Feed it anything: a `script`
recording, a tmux `capture-pane -e` dump, or clipring's own output.

## Terminal support (tested manually, 2026-07)

| Terminal | OSC 52 set | Notes |
|---|---|---|
| xterm | yes | `allowWindowOps` resource must permit it |
| kitty | yes | on by default, generous size limit |
| WezTerm | yes | on by default |
| Alacritty | yes | on by default |
| foot | yes | on by default |
| iTerm2 | yes | enable "Applications in terminal may access clipboard" |
| Windows Terminal | yes | on by default |
| tmux (outer) | yes | `set -g allow-passthrough on`, or `set-clipboard on` |
| GNU screen | passthrough only | needs the chunked DCS envelope above |

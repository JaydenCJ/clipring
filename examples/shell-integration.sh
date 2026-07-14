# Shell helpers for clipring — copy the ones you like into ~/.bashrc or
# ~/.zshrc. Nothing here runs on its own; this file is documentation you
# can source.

# yy — copy stdin, or the arguments if given.
#   git rev-parse HEAD | yy
#   yy "kubectl get pods -A"
yy() {
    if [ "$#" -gt 0 ]; then
        clipring copy --trim -- "$@"
    else
        clipring copy --trim
    fi
}

# pp — paste history entry N (default 0, the newest) to stdout.
#   pp | psql
#   pp 2 > snippet.sql
pp() {
    clipring paste "${1:-0}"
}

# yl — interactive picker: choose an entry by number, re-copy it to the
# terminal clipboard, and promote it to the front of the ring.
yl() {
    clipring pick
}

# Bonus for bash: Alt-y copies the current command line into the ring
# without executing it.
#   bind -x '"\ey": "printf %s \"$READLINE_LINE\" | clipring copy"'

#!/bin/sh
# Seed a throwaway clipring state directory with realistic entries and walk
# through list / search / paste / decode. Never touches your real history.
set -eu

CLIPRING="${CLIPRING:-clipring}"
STATE=$(mktemp -d "${TMPDIR:-/tmp}/clipring-demo.XXXXXX")
trap 'rm -rf "$STATE"' EXIT
export CLIPRING_STATE="$STATE"

echo "# seeding four entries (--no-emit: store only, no terminal writes)"
printf 'ssh -L 5432:127.0.0.1:5432 deploy@example.test' | "$CLIPRING" copy --no-emit
"$CLIPRING" copy --no-emit "kubectl logs -f api-7d4b9c --tail=100"
printf "SELECT id, email FROM users WHERE created_at > now() - interval '7 days';" \
  | "$CLIPRING" copy --no-emit
"$CLIPRING" copy --no-emit "export DEPLOY_TOKEN=redacted-for-demo"
"$CLIPRING" pin 0

echo
echo "# clipring list"
"$CLIPRING" list

echo
echo "# clipring search kubectl"
"$CLIPRING" search kubectl

echo
echo "# clipring paste 1   (byte-identical to what went in)"
"$CLIPRING" paste 1
echo

echo
echo "# emit | decode round trip (what your terminal would receive)"
printf 'round trip' | "$CLIPRING" emit --stdout | "$CLIPRING" decode
echo

echo
echo "# done — temporary state dir is removed on exit"

#!/usr/bin/env bash
# Local Gotify for tests.
#   tests/seed.sh up                      start Gotify, create app "Uptime Kuma", print APP_TOKEN
#   tests/seed.sh send <priority> <text>  send a message as Uptime Kuma would
#   tests/seed.sh down                    stop and discard everything
set -euo pipefail
cd "$(dirname "$0")"
URL=http://127.0.0.1:18080
TOKEN_FILE=.app-token
# Throwaway login of the local test container (see docker-compose.yml).
TEST_USER=admin
TEST_PASS=admin

case "${1:-}" in
  up)
    docker compose up -d --quiet-pull >/dev/null
    for _ in $(seq 60); do curl -fsS "$URL/health" >/dev/null 2>&1 && break; sleep 0.5; done
    curl -fsS "$URL/health" >/dev/null
    if [ ! -s "$TOKEN_FILE" ] || ! curl -fsS -H "X-Gotify-Key: $(cat "$TOKEN_FILE")" -X POST "$URL/message" \
         -H 'Content-Type: application/json' -d '{"message":"seed check","priority":0}' >/dev/null 2>&1; then
      curl -fsS --user "$TEST_USER:$TEST_PASS" -H 'Content-Type: application/json' -d '{"name":"Uptime Kuma"}' "$URL/application" \
        | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["token"])' > "$TOKEN_FILE"
    fi
    echo "GOTIFY_TEST_URL=$URL"
    echo "APP_TOKEN=$(cat "$TOKEN_FILE")"
    ;;
  send)
    prio=$2; text=$3
    python3 -I -c 'import json,sys; print(json.dumps({"title":"Uptime-Kuma","message":sys.argv[2],"priority":int(sys.argv[1])}))' "$prio" "$text" \
      | curl -fsS -H "X-Gotify-Key: $(cat "$TOKEN_FILE")" -H 'Content-Type: application/json' -d @- "$URL/message" >/dev/null
    ;;
  down)
    docker compose down -v >/dev/null; rm -f "$TOKEN_FILE"
    ;;
  *) echo "usage: $0 up|send <priority> <text>|down" >&2; exit 2 ;;
esac

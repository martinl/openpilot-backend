#!/bin/sh
set -eu

API_HOST="${API_ENDPOINT:-https://api.apps.k8s.mlp.ee}"
ATHENA_HOST="${ATHENA_ENDPOINT:-https://athena.apps.k8s.mlp.ee}"
ATHENA_WS_HOST="$(printf '%s' "${ATHENA_HOST}" | sed 's|^https://|wss://|; s|^http://|ws://|')"
GITHUB_CLIENT_ID="${GITHUB_CLIENT:-}"

cat <<JSEOF > /usr/share/nginx/html/config.js
window.__OPENPILOT_CONFIG__ = {
  API_HOST: "${API_HOST}",
  ATHENA_HOST: "${ATHENA_HOST}",
  GITHUB_CLIENT_ID: "${GITHUB_CLIENT_ID}"
};
JSEOF

find /usr/share/nginx/html -type f \( -name '*.js' -o -name '*.html' \) -exec sed -i \
  -e "s|__OPENPILOT_API_HOST__|${API_HOST}|g" \
  -e "s|__OPENPILOT_ATHENA_HOST__|${ATHENA_HOST}|g" \
  -e "s|__OPENPILOT_GITHUB_CLIENT_ID__|${GITHUB_CLIENT_ID}|g" \
  -e "s|https://api\.konik\.ai|${API_HOST}|g" \
  -e "s|https://athena\.konik\.ai|${ATHENA_HOST}|g" \
  -e "s|wss://athena\.konik\.ai|${ATHENA_WS_HOST}|g" \
  {} +

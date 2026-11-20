#!/bin/sh
set -eu

API_HOST="${API_ENDPOINT:-http://localhost:3112}"
ATHENA_HOST="${ATHENA_ENDPOINT:-http://localhost:3112}"
ATHENA_WS_HOST="$(printf '%s' "${ATHENA_HOST}" | sed 's|^https://|wss://|; s|^http://|ws://|')"
USERADMIN_HOST_URL="${USERADMIN_URL:-http://localhost:3112}"
GITHUB_CLIENT_ID="${GITHUB_CLIENT:-}"
MAPBOX_ACCESS_TOKEN="${MAPBOX_TOKEN:-}"

cat <<JSEOF > /usr/share/nginx/html/config.js
window.__APP_CONFIG__ = {
  apiUrl: "${API_HOST}",
  athenaUrl: "${ATHENA_HOST}",
  useradminUrl: "${USERADMIN_HOST_URL}",
  githubClientId: "${GITHUB_CLIENT_ID}",
  mapboxToken: "${MAPBOX_ACCESS_TOKEN}"
};
window.__OPENPILOT_CONFIG__ = {
  API_HOST: "${API_HOST}",
  ATHENA_HOST: "${ATHENA_HOST}",
  USERADMIN_URL: "${USERADMIN_HOST_URL}",
  GITHUB_CLIENT_ID: "${GITHUB_CLIENT_ID}",
  MAPBOX_TOKEN: "${MAPBOX_ACCESS_TOKEN}"
};
JSEOF

find /usr/share/nginx/html -type f \( -name '*.js' -o -name '*.html' \) -exec sed -i \
  -e "s|__OPENPILOT_API_HOST__|${API_HOST}|g" \
  -e "s|__OPENPILOT_ATHENA_HOST__|${ATHENA_HOST}|g" \
  -e "s|__OPENPILOT_GITHUB_CLIENT_ID__|${GITHUB_CLIENT_ID}|g" \
  -e "s|https://api\.konik\.ai|${API_HOST}|g" \
  -e "s|https://athena\.konik\.ai|${ATHENA_HOST}|g" \
  -e "s|https://useradmin\.konik\.ai|${USERADMIN_HOST_URL}|g" \
  -e "s|wss://athena\.konik\.ai|${ATHENA_WS_HOST}|g" \
  {} +

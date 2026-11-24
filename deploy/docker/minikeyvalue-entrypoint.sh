#!/bin/sh
set -eu

MKV_VOLUME_HOST="${MKV_VOLUME_HOST:-localhost}"
MKV_REPLICAS="${MKV_REPLICAS:-1}"
MKV_VOLTIMEOUT="${MKV_VOLTIMEOUT:-30s}"
MKV_DATA_DIR="${MKV_DATA_DIR:-/data}"
MKV_DB="${MKV_DB:-${MKV_DATA_DIR}/indexdb/}"
MKV_NGINX_WORKERS="${MKV_NGINX_WORKERS:-2}"

mkdir -p "$MKV_DB" /tmp/nginx

CONF="/tmp/nginx/minikeyvalue.conf"
cat > "$CONF" <<EOF
daemon off;
worker_processes ${MKV_NGINX_WORKERS};
pcre_jit on;

error_log /dev/stderr error;
pid /tmp/nginx/nginx.pid;

events {
  multi_accept on;
  accept_mutex off;
  worker_connections 4096;
}

http {
  sendfile on;
  sendfile_max_chunk 1024k;
  tcp_nopush on;
  tcp_nodelay on;
  open_file_cache off;
  types_hash_max_size 2048;
  server_tokens off;
  default_type application/octet-stream;
  access_log off;
  client_body_temp_path /tmp/nginx/client_temp;
  proxy_temp_path /tmp/nginx/proxy_temp;
  fastcgi_temp_path /tmp/nginx/fastcgi_temp;
  uwsgi_temp_path /tmp/nginx/uwsgi_temp;
  scgi_temp_path /tmp/nginx/scgi_temp;
EOF

VOLUMES=""
for i in 1 2 3 4 5; do
  port=$((3000 + i))
  vol="${MKV_DATA_DIR}/volume${i}"
  mkdir -p "${vol}/body_temp"
  cat >> "$CONF" <<EOF

  server {
    listen ${port} default_server backlog=4096;
    location / {
      root ${vol};
      disable_symlinks off;
      client_body_temp_path ${vol}/body_temp;
      client_max_body_size 0;
      dav_methods PUT DELETE;
      dav_access group:rw all:r;
      create_full_put_path on;
      autoindex on;
      autoindex_format json;
    }
  }
EOF
  if [ -n "$VOLUMES" ]; then
    VOLUMES="${VOLUMES},${MKV_VOLUME_HOST}:${port}"
  else
    VOLUMES="${MKV_VOLUME_HOST}:${port}"
  fi
done

echo "}" >> "$CONF"

nginx -c "$CONF" -p /tmp/nginx -e /dev/stderr &

exec /mkv/mkv \
  -port 3000 \
  -volumes "$VOLUMES" \
  -replicas "$MKV_REPLICAS" \
  -db "$MKV_DB" \
  -voltimeout "$MKV_VOLTIMEOUT" \
  -v \
  server

#!/bin/sh
set -eu

api_base="${PUBLIC_API_BASE_URL:-http://localhost:8080}"

case "$api_base" in
	http://*|https://*) ;;
	*) echo "PUBLIC_API_BASE_URL must start with http:// or https://" >&2; exit 1 ;;
esac

if printf '%s' "$api_base" | grep -q '[[:space:]]'; then
	echo "PUBLIC_API_BASE_URL must not contain whitespace" >&2
	exit 1
fi

escaped_api_base=$(printf '%s' "$api_base" | sed 's/\\/\\\\/g; s/"/\\"/g')

cat > /usr/share/caddy/env.js <<EOF
window.__DEEPREF_CONFIG__ = { apiBaseUrl: "$escaped_api_base" };
EOF

exec "$@"

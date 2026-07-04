#!/usr/bin/env bash
set -e

ROOT="$(cd "$(dirname "$0")" && pwd)"
PORT=8080
URL="http://127.0.0.1:$PORT"

# Force-kill anything on the port
pids=$(lsof -ti tcp:$PORT 2>/dev/null || true)
if [ -n "$pids" ]; then
  echo "Freeing port $PORT..."
  echo "$pids" | xargs kill -9 2>/dev/null || true
  sleep 1
fi

# Build frontend
echo "Building frontend..."
cd "$ROOT/frontends"
[ ! -d node_modules ] && npm install
npm run build

# Start server in background
echo "Starting server..."
cd "$ROOT"
cargo run --bin server &
SERVER_PID=$!

# Wait until server responds, then open browser
echo "Waiting for server at $URL..."
for i in $(seq 1 30); do
  if curl -s --max-time 1 "$URL" > /dev/null 2>&1; then
    echo "Server ready — opening browser"
    open "$URL"
    break
  fi
  sleep 0.5
done

# Ctrl+C kills the server
trap "kill $SERVER_PID 2>/dev/null" EXIT
wait $SERVER_PID

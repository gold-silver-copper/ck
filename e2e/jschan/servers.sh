#!/usr/bin/env bash
# Real jschan servers for ck's account test (src/post/jschan_e2e.rs): upstream jschan and
# ptchina (ptchan.org's fork), each with its own Redis and database, seeded with the accounts
# and boards the test uses (seed.js). Everything lives under $CK_E2E (default
# ~/.cache/ck-e2e): nothing is installed system-wide, but for the image tools with --packages.
# The same script runs on Linux, macOS and Windows (inside WSL1's Ubuntu).
#
#   e2e/jschan/servers.sh install [--packages]   Node, MongoDB, Redis and both servers
#   e2e/jschan/servers.sh start                   start, reset and seed them
#   e2e/jschan/servers.sh stop
#
# Then (each on a host of its own, so their cookies stay apart), as src/post/jschan_e2e.rs says:
#   CK_E2E_JSCHAN="http://127.0.0.1:7100 http://localhost:7101" cargo test -- --ignored jschan_accounts
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
CK_E2E=${CK_E2E:-$HOME/.cache/ck-e2e}
NODE_VERSION=20.18.0
MONGO_VERSION=7.0.14
REDIS_VERSION=7.2.5
MONGO_PORT=27117
# name, git URL, pinned commit, port, Redis port
SERVERS=(
  "jschan https://gitgud.io/fatchan/jschan.git c0d50eb168501d7e0ac8fd9c516901c6dbfc5270 7100 6479"
  "ptchina https://github.com/l29utp0/ptchina.git 045d02d641acb1949e7f71916f4b465f3f950328 7101 6480"
)

os=$(uname -s)
arch=$(uname -m)
export PATH="$CK_E2E/node/bin:$CK_E2E/mongodb/bin:$CK_E2E/redis/src:$PATH"

# Download $1 into $2 (a folder, made fresh), unpacking a .tar.gz/.tgz one level down.
fetch() {
  rm -rf "$2" && mkdir -p "$2"
  curl -fsSL --retry 3 "$1" | tar -xz -C "$2" --strip-components=1
}

install() {
  mkdir -p "$CK_E2E"
  if [ "${1:-}" = --packages ]; then
    # ImageMagick draws the captchas; fonts for its text.
    if [ "$os" = Darwin ]; then
      brew install imagemagick
    else
      sudo=$([ "$(id -u)" = 0 ] && echo "" || echo sudo)
      $sudo apt-get update -q
      $sudo apt-get install -yq imagemagick fonts-dejavu-core build-essential curl git
    fi
  fi
  local n_os n_arch m_name
  case "$os-$arch" in
    Linux-x86_64) n_os=linux n_arch=x64 m_name=mongodb-linux-x86_64-ubuntu2204 ;;
    Linux-aarch64) n_os=linux n_arch=arm64 m_name=mongodb-linux-aarch64-ubuntu2204 ;;
    Darwin-arm64) n_os=darwin n_arch=arm64 m_name=mongodb-macos-arm64 ;;
    Darwin-x86_64) n_os=darwin n_arch=x64 m_name=mongodb-macos-x86_64 ;;
    *) echo "no builds for $os-$arch" >&2; exit 1 ;;
  esac
  [ -x "$CK_E2E/node/bin/node" ] || fetch "https://nodejs.org/dist/v$NODE_VERSION/node-v$NODE_VERSION-$n_os-$n_arch.tar.gz" "$CK_E2E/node"
  [ -x "$CK_E2E/mongodb/bin/mongod" ] || fetch "https://fastdl.mongodb.org/${n_os/darwin/osx}/$m_name-$MONGO_VERSION.tgz" "$CK_E2E/mongodb"
  if [ ! -x "$CK_E2E/redis/src/redis-server" ]; then
    fetch "https://download.redis.io/releases/redis-$REDIS_VERSION.tar.gz" "$CK_E2E/redis"
    make -s -C "$CK_E2E/redis" -j3 BUILD_TLS=no MALLOC=libc >/dev/null
  fi
  for s in "${SERVERS[@]}"; do
    read -r name url commit port redis <<<"$s"
    local dir="$CK_E2E/src/$name"
    if [ "$(git -C "$dir" rev-parse HEAD 2>/dev/null)" != "$commit" ]; then
      rm -rf "$dir" && git clone -q "$url" "$dir" && git -C "$dir" checkout -q "$commit"
    fi
    # Its secrets: this database, this Redis, this port.
    sed -e "s|dbURL: .*|dbURL: 'mongodb://127.0.0.1:$MONGO_PORT',|" -e "s|dbName: .*|dbName: '$name',|" \
        -e "s|port: '6379'|port: '$redis'|" -e "s|CHANGE-ME-YOUR-SECURE-REDIS-PASSWORD|ck-e2e|" -e "s|port: 7000|port: $port|" \
        -e "s|debugLogs: true|debugLogs: false|" "$dir/configs/secrets.js.example" > "$dir/configs/secrets.js"
    # The favicon step asks a web service for them: none, offline.
    : > "$dir/gulp/res/icons/html_code.html"
    # (Upstream's lock file is out of step with its package.json, which `npm ci` refuses.)
    (cd "$dir" && npm install --no-audit --no-fund --loglevel=error)
  done
}

# Wait until $1 answers on 127.0.0.1:$2, or fail saying so.
wait_port() {
  for _ in $(seq 120); do
    if (exec 3<>"/dev/tcp/127.0.0.1/$2") 2>/dev/null; then return 0; fi
    sleep 1
  done
  echo "$1 didn't start on port $2" >&2
  return 1
}

start() {
  local run="$CK_E2E/run"
  mkdir -p "$run/db"
  mongod --dbpath "$run/db" --port "$MONGO_PORT" --bind_ip 127.0.0.1 --logpath "$run/mongod.log" --pidfilepath "$run/mongod.pid" --fork >/dev/null
  wait_port MongoDB "$MONGO_PORT"
  for s in "${SERVERS[@]}"; do
    read -r name url commit port redis <<<"$s"
    local dir="$CK_E2E/src/$name"
    redis-server --port "$redis" --bind 127.0.0.1 --requirepass ck-e2e --save '' --appendonly no --daemonize yes \
      --logfile "$run/redis-$name.log" --pidfile "$run/redis-$name.pid" --dir "$run"
    wait_port "Redis ($name)" "$redis"
    # A fresh database (it prints the admin's password, unused here), then the accounts.
    (cd "$dir" && npx gulp reset >"$run/reset-$name.log" 2>&1) || { tail -n 30 "$run/reset-$name.log"; exit 1; }
    node "$HERE/seed.js" accounts "$dir"
    # Not production: Node serves the pages and files itself, with no nginx in front.
    for js in server worker; do
      (cd "$dir" && NODE_ENV=development exec node "$js.js" </dev/null >"$run/$name-$js.log" 2>&1) &
      echo $! >"$run/$name-$js.pid"
    done
    wait_port "$name" "$port" || { tail -n 30 "$run/$name-server.log"; exit 1; }
    node "$HERE/seed.js" boards "$dir" "http://127.0.0.1:$port"
    echo "$name: http://127.0.0.1:$port"
  done
}

# Stop what start started, and wait till it's gone (a new MongoDB can't have the port before).
stop() {
  local run="$CK_E2E/run" pids=()
  for f in "$run"/*.pid; do
    [ -e "$f" ] && pids+=("$(cat "$f")")
  done
  # (Not "${pids[@]}" when there are none: macOS's bash 3 calls that unbound.)
  if [ ${#pids[@]} -gt 0 ]; then
    kill "${pids[@]}" 2>/dev/null || true
    for _ in $(seq 60); do
      kill -0 "${pids[@]}" 2>/dev/null || break
      sleep 0.5
    done
  fi
  rm -rf "$run"
}

case "${1:-}" in
  install) install "${2:-}" ;;
  start) start ;;
  stop) stop ;;
  *) echo "usage: $0 install [--packages] | start | stop" >&2; exit 2 ;;
esac

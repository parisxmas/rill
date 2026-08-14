#!/bin/bash
# HTTP server benchmark: Rill vs Go's net/http, both serving the same one-route
# "Hello, World!" service. Every server is driven twice by wrk — once with a
# fresh connection per request, once with keep-alive — because the two modes
# stress different things: accept + per-connection setup, vs. request parsing
# and response writing on an established connection.
#
# Both servers keep connections alive and let the client decide, so the mode is
# selected purely by wrk's `Connection:` header — no server is configured into a
# different code path than the other.
#
# Longer runs measure lower for both servers, so the ratio between them is the
# meaningful number rather than the absolute one — see `drain` below for why.
set -u
DUR=${DUR:-10}
CONNS=${CONNS:-50}
THREADS=${THREADS:-4}
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)

if [ ! -x "$here/rill_server" ] || [ "$here/rill_server.rill" -nt "$here/rill_server" ]; then
  (cd "$root" && cargo run --quiet --release --bin rill -- \
     build benchmarks/http/rill_server.rill -o benchmarks/http/rill_server) || exit 1
fi
if [ ! -x "$here/go_server" ] || [ "$here/go_server.go" -nt "$here/go_server" ]; then
  (cd "$here" && go build -o go_server go_server.go) || exit 1
fi
command -v wrk >/dev/null || { echo "wrk is not installed (brew install wrk)"; exit 1; }

# Connections closed by the server sit in TIME_WAIT for ~30s, and a
# connection-per-request run leaves hundreds of thousands of them. They slow
# down every later case, not just the one on the same port, so wait for the
# kernel to clear them before each measurement.
drain() {
  local i=0 n
  while :; do
    n=$(netstat -an -p tcp 2>/dev/null | grep -c "\.81[0-9][0-9] .*TIME_WAIT")
    [ "$n" -lt 1000 ] && break
    [ $i -ge 90 ] && break
    sleep 2; i=$((i + 2))
  done
  sleep 1
}

run_case() { # name  port  close?  command...
  local name=$1 port=$2 close=$3; shift 3
  drain
  "$@" >/dev/null 2>&1 &
  local pid=$!
  sleep 2
  local wrk_args=(-t"$THREADS" -c"$CONNS" -d"${DUR}s" --latency)
  [ "$close" = yes ] && wrk_args+=(-H "Connection: close")
  local out rps lat p99 mem err
  out=$(wrk "${wrk_args[@]}" "http://127.0.0.1:$port/" 2>/dev/null)
  rps=$(echo "$out" | awk '/Requests\/sec/{printf "%.0f", $2}')
  lat=$(echo "$out" | awk '/^ *Latency +[0-9]/{print $2}')
  p99=$(echo "$out" | awk '/^ *99%/{print $2}')
  err=$(echo "$out" | sed -n 's/ *Socket errors: //p')
  mem=$(ps -o rss= -p "$pid" 2>/dev/null | awk '{printf "%.1f MB", $1/1024}')
  kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
  printf "%-30s %10s %10s %10s %10s   %s\n" \
    "$name" "${rps:-failed}" "${lat:-n/a}" "${p99:-n/a}" "${mem:-n/a}" "${err:-}"
}

echo "wrk -t$THREADS -c$CONNS -d${DUR}s, GET / -> \"Hello, World!\""
printf "%-30s %10s %10s %10s %10s\n" "server" "req/sec" "avg lat" "p99 lat" "RSS"
echo "------------------------------------------------------------------------------"
run_case "rill (keep-alive)"          8100 no  "$here/rill_server"
run_case "go   (keep-alive)"          8103 no  "$here/go_server" -port 8103
run_case "rill (connection per req)"  8100 yes "$here/rill_server"
run_case "go   (connection per req)"  8101 yes "$here/go_server" -port 8101

#!/usr/bin/env bash
# watch_trades.sh — live tail of closed trades from Scylla.
#
# Runs in a second terminal alongside the scheduler. Polls the reports
# table every N seconds; prints one line per newly-closed trade since the
# previous poll, plus a session-total footer.
#
# Usage:
#   ./watch_trades.sh                # 30s polls, default keyspace/container
#   ./watch_trades.sh 10             # 10s polls
#
# Ctrl-C to stop. Read-only; safe to run multiple copies.

set -u

INTERVAL="${1:-30}"
CONTAINER="${SCYLLA_CONTAINER:-scylla}"
KEYSPACE="${KEYSPACE:-okx}"
LIMIT="${LIMIT:-200}" # rows pulled per poll; must exceed max trades/interval

# Session totals (running since script start, not since scheduler start).
session_trades=0
session_profit=0
session_wins=0

# Dedupe key file — one line per trade already seen, cleared on Ctrl-C.
SEEN="$(mktemp)"
trap 'rm -f "$SEEN"; echo; echo "stopped."; exit 0' INT TERM EXIT

# --- preflight -------------------------------------------------------------

if ! docker exec "$CONTAINER" cqlsh -e "SELECT release_version FROM system.local" \
  >/dev/null 2>&1; then
  echo "ERROR: cannot reach Scylla via 'docker exec $CONTAINER cqlsh'." >&2
  echo "Is the container running? Override with SCYLLA_CONTAINER=<name>." >&2
  exit 1
fi

echo "watching $KEYSPACE.reports every ${INTERVAL}s (Ctrl-C to stop)"
echo "─────────────────────────────────────────────────────────────────────"

# --- main loop -------------------------------------------------------------

while true; do
  # Pull recent rows. PK is (round_id, instid) so we can't ORDER BY ts at
  # query time across partitions; sort client-side, dedupe by natural key.
  raw="$(docker exec "$CONTAINER" cqlsh -e "
        SELECT round_id, instid, reason, earnings, change, time_left, ts
          FROM $KEYSPACE.reports LIMIT $LIMIT;
    " 2>/dev/null | sed 's/\r$//')"

  # awk pipeline:
  #   - skip cqlsh header/separator/footer lines
  #   - build dedupe key round_id|instid|ts
  #   - emit only rows whose key isn't already in $SEEN
  #   - format for the human, mark with symbol by outcome
  new_lines="$(awk -F'|' -v seen="$SEEN" '
        BEGIN {
            while ((getline line < seen) > 0) already[line] = 1;
            close(seen);
        }
        NR > 3 && NF >= 7 {
            for (i = 1; i <= NF; i++) gsub(/^ +| +$/, "", $i);
            if ($1 == "" || $1 == "round_id") next;
            key = $1 "|" $2 "|" $7;
            if (already[key]) next;
            already[key] = 1;
            print key >> seen;

            round_id = $1; instid = $2; reason = $3;
            earnings = $4 + 0; change = $5 + 0; held = $6 + 0; ts = $7;

            # Trailing 3 chars of ts are timezone; keep HH:MM:SS from position 12-19.
            hhmmss = substr(ts, 12, 8);

            # Held time in Mm SSs. `time_left` is seconds remaining at exit;
            # timeout - time_left = seconds actually held. Scheduler timeout
            # can vary by strategy, so we just report time_left.
            if (held < 0) held = 0;
            mm = int(held / 60); ss = held % 60;
            held_str = sprintf("%dm%02ds_left", mm, ss);

            # Outcome glyph
            mark = "·";  # dim: timeout with tiny move
            if (reason == "cashout") mark = "★";
            else if (reason == "stoploss") mark = "✗";
            else if (reason == "floor_reached") mark = (earnings > 0) ? "✓" : "·";
            else if (earnings > 0.05) mark = "✓";
            else if (earnings < -0.10) mark = "✗";

            printf "%s %s %-14s %-14s %+8.3f$  chg %+6.2f%%  %s  #%d\n",
                   hhmmss, mark, instid, reason, earnings, change, held_str, round_id;
        }
    ' <<<"$raw")"

  # Emit new trades (if any) and update session totals.
  if [[ -n "$new_lines" ]]; then
    echo "$new_lines"

    while IFS= read -r line; do
      # earnings is field 5 (after the sprintf); pluck it with awk.
      e="$(awk '{ for (i=1;i<=NF;i++) if ($i ~ /\$$/) { gsub(/\$/,"",$i); print $i; exit } }' <<<"$line")"
      [[ -z "$e" ]] && continue
      session_trades=$((session_trades + 1))
      session_profit="$(awk -v a="$session_profit" -v b="$e" 'BEGIN{printf "%.4f", a+b}')"
      if awk -v e="$e" 'BEGIN{exit !(e > 0)}'; then
        session_wins=$((session_wins + 1))
      fi
    done <<<"$new_lines"

    # Footer: session totals only when there are new trades to summarize.
    if [[ "$session_trades" -gt 0 ]]; then
      pct="$(awk -v w="$session_wins" -v n="$session_trades" \
        'BEGIN{printf "%d", 100*w/n}')"
      printf "── session: %d trades  net %+.3f\$  wins %d (%d%%) ──\n" \
        "$session_trades" "$session_profit" "$session_wins" "$pct"
    fi
  fi

  sleep "$INTERVAL"
done

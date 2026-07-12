#!/usr/bin/env bash
# analyze_reports.sh — pull scheduler report stats from Scylla into one file.
#
# Usage:
#   ./analyze_reports.sh                    # default: writes ./analysis-<timestamp>.txt
#   ./analyze_reports.sh /tmp/report.txt    # custom path
#
# No dependencies beyond docker + awk + sort. Safe to re-run; read-only.

set -u # not -e: some queries are allowed to return empty without aborting

OUT="${1:-./analysis-$(date -u +%Y%m%d-%H%M%S).txt}"
CONTAINER="${SCYLLA_CONTAINER:-scylla}"
KEYSPACE="${KEYSPACE:-okx}"

# All raw queries dumped here; kept for grepping if a section looks off.
RAW="$(mktemp -d)"
trap 'rm -rf "$RAW"' EXIT

# --- helpers ---------------------------------------------------------------

section() { printf '\n\n=== %s ===\n\n' "$1" | tee -a "$OUT" >/dev/null; }
line() { printf '%s\n' "$1" | tee -a "$OUT" >/dev/null; }

# Run cqlsh; strip control chars; append to output. On failure, note it and
# keep going — a broken query shouldn't kill the whole report.
cql() {
  local q="$1"
  docker exec "$CONTAINER" cqlsh -e "$q" 2>&1 |
    sed 's/\r$//' |
    tee -a "$OUT" >/dev/null ||
    line "[query failed — see above]"
}

# Dump a query's raw output to a temp file for downstream awk parsing.
cql_raw() {
  local q="$1" dest="$2"
  docker exec "$CONTAINER" cqlsh -e "$q" 2>/dev/null |
    sed 's/\r$//' >"$dest"
}

# --- preflight -------------------------------------------------------------

: >"$OUT"
line "scheduler report analysis"
line "generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
line "container: $CONTAINER   keyspace: $KEYSPACE"

if ! docker exec "$CONTAINER" cqlsh -e "SELECT release_version FROM system.local" \
  >/dev/null 2>&1; then
  line ""
  line "ERROR: cannot reach Scylla via 'docker exec $CONTAINER cqlsh'."
  line "Is the container running? Override with SCYLLA_CONTAINER=<name>."
  exit 1
fi

# --- 1. headline numbers ---------------------------------------------------

section "1. headline"
cql "SELECT COUNT(*), AVG(earnings), SUM(earnings), MIN(earnings), MAX(earnings)
       FROM $KEYSPACE.reports;"

# --- 2. reason distribution ------------------------------------------------
# GROUP BY doesn't work here because 'reason' isn't in the primary key, so
# we pull the raw column and count client-side.

section "2. reason distribution"
cql_raw "SELECT reason FROM $KEYSPACE.reports LIMIT 20000;" "$RAW/reasons.txt"
awk '
/^ *(timeout|floor_reached|stoploss|cashout|low_volume|low_change|None) *$/ {
    gsub(/ /, ""); n[$0]++; total++
}
END {
    if (total == 0) { print "no rows"; exit }
    printf "%-16s %6s %8s\n", "reason", "n", "pct";
    for (k in n) printf "%-16s %6d %7.1f%%\n", k, n[k], 100*n[k]/total;
    printf "%-16s %6d %7.1f%%\n", "TOTAL", total, 100.0;
}' "$RAW/reasons.txt" | sort -k2 -rn | tee -a "$OUT" >/dev/null

# --- 3. reason × earnings --------------------------------------------------

section "3. earnings breakdown by exit reason"
cql_raw "SELECT round_id, instid, reason, earnings, change, time_left, highest, lowest
           FROM $KEYSPACE.reports LIMIT 20000;" "$RAW/reports.txt"

awk -F'|' '
NR > 3 && NF >= 8 {
    for (i = 1; i <= NF; i++) gsub(/ /, "", $i);
    reason = $3;
    if (reason == "" || reason == "reason") next;
    n[reason]++;
    sum[reason]   += $4;
    ch[reason]    += $5;
    tl[reason]    += $6;
    hi[reason]    += $7;
    lo[reason]    += $8;
}
END {
    if (length(n) == 0) { print "no rows"; exit }
    printf "%-15s %6s %11s %10s %10s %10s %10s %10s\n",
           "reason", "n", "total_$", "avg_$", "avg_chg%", "avg_time_s", "avg_hi%", "avg_lo%";
    for (k in n)
        printf "%-15s %6d %11.3f %10.4f %10.4f %10.1f %10.4f %10.4f\n",
               k, n[k], sum[k], sum[k]/n[k], ch[k]/n[k], tl[k]/n[k], hi[k]/n[k], lo[k]/n[k];
}' "$RAW/reports.txt" | sort -k3 -gr | tee -a "$OUT" >/dev/null

# --- 4. per-token performance ---------------------------------------------

section "4a. best tokens (by total earnings)"
awk -F'|' '
NR > 3 && NF >= 4 {
    for (i = 1; i <= NF; i++) gsub(/ /, "", $i);
    inst = $2;
    if (inst == "" || inst == "instid") next;
    n[inst]++; sum[inst] += $4;
}
END {
    if (length(n) == 0) { print "no rows"; exit }
    printf "%-16s %6s %11s %10s\n", "instid", "n", "total_$", "avg_$";
    for (k in n) printf "%-16s %6d %11.3f %10.4f\n", k, n[k], sum[k], sum[k]/n[k];
}' "$RAW/reports.txt" | sort -k3 -gr | head -20 | tee -a "$OUT" >/dev/null

section "4b. worst tokens (by total earnings)"
awk -F'|' '
NR > 3 && NF >= 4 {
    for (i = 1; i <= NF; i++) gsub(/ /, "", $i);
    inst = $2;
    if (inst == "" || inst == "instid") next;
    n[inst]++; sum[inst] += $4;
}
END {
    if (length(n) == 0) { print "no rows"; exit }
    printf "%-16s %6s %11s %10s\n", "instid", "n", "total_$", "avg_$";
    for (k in n) printf "%-16s %6d %11.3f %10.4f\n", k, n[k], sum[k], sum[k]/n[k];
}' "$RAW/reports.txt" | sort -k3 -g | head -15 | tee -a "$OUT" >/dev/null

# --- 5. hourly earnings ---------------------------------------------------
# Ts column format is `YYYY-MM-DD HH:MM:SS.sss+0000`; hour is `YYYY-MM-DD HH`.

section "5. earnings by hour (UTC)"
cql_raw "SELECT ts, earnings FROM $KEYSPACE.reports LIMIT 20000;" "$RAW/hourly.txt"
awk -F'|' '
NR > 3 && NF >= 2 {
    ts = $1; gsub(/^ +| +$/, "", ts);
    if (ts == "" || ts == "ts") next;
    hour = substr(ts, 1, 13);
    e = $2; gsub(/ /, "", e);
    n[hour]++; sum[hour] += e;
}
END {
    if (length(n) == 0) { print "no rows"; exit }
    printf "%-14s %6s %11s %10s\n", "hour_utc", "n", "total_$", "avg_$";
    for (k in n) printf "%-14s %6d %11.3f %10.4f\n", k, n[k], sum[k], sum[k]/n[k];
}' "$RAW/hourly.txt" | sort | tee -a "$OUT" >/dev/null

# --- 6. winning trades ----------------------------------------------------

section "6. winning trades (earnings > 0.05)"
cql_raw "SELECT instid, reason, earnings, change, time_left, highest, lowest
           FROM $KEYSPACE.reports LIMIT 20000;" "$RAW/wins.txt"
{
  head -3 "$RAW/wins.txt"
  awk -F'|' 'NR > 3 && NF >= 3 {
        e = $3; gsub(/ /, "", e);
        if (e+0 > 0.05) print
    }' "$RAW/wins.txt" | sort -t'|' -k3 -gr | head -30
} | tee -a "$OUT" >/dev/null

# --- 7. all active strategy configs the reports were generated under ------
# If configure_from_report was accumulating history against multiple hashes,
# useful to know for interpreting mixed results.

section "7. strategy configs present in the reports table"
cql_raw "SELECT strategy FROM $KEYSPACE.reports LIMIT 20000;" "$RAW/strategies.txt"
awk 'NR > 3 && /^ *[0-9a-f]/ { gsub(/ /, ""); n[$0]++ }
END { for (k in n) printf "%s  reports=%d\n", k, n[k] }' \
  "$RAW/strategies.txt" | sort -k2 -t= -rn | tee -a "$OUT" >/dev/null

section "7b. active strategies table (config that produced each hash)"
cql "SELECT hash, timeframe, timeout, cooldown, min_change, min_vol,
            min_rising_candles, sell_floor, stoploss, cashout, min_change_last_candle
       FROM $KEYSPACE.strategies LIMIT 20;"

# --- footer ---------------------------------------------------------------

section "done"
line "wrote: $OUT"
printf '\nreport ready: %s\n' "$OUT"

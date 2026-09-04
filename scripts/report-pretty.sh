#!/usr/bin/env bash
# analyze_reports.sh — pull scheduler report stats from Scylla into one file.
#
# Usage:
#   ./analyze_reports.sh                    # default: writes ./analysis-<timestamp>.txt
#   ./analyze_reports.sh /tmp/report.txt    # custom path
#
# No dependencies beyond docker + awk + sort. Safe to re-run; read-only.
#
# Sections:
#   1. Headline totals
#   2. Reason distribution
#   3. Earnings breakdown by reason (avg_hi / avg_lo — the key diagnostic)
#   4. Per-token sequence (chronological W/L pattern per frequently-traded token)
#   5. Loss shape (bad-entry vs bad-exit — tells us which side to fix)
#   6. Timeout dissection (of timeouts specifically, how many touched a win level)
#   7. Winning trades (top 15 by earnings)
#   8. Strategy configs present in reports (blend detection across tuning runs)

set -u # not -e: individual queries may return empty without aborting

OUT="${1:-./analysis-$(date -u +%Y%m%d-%H%M%S).txt}"
CONTAINER="${SCYLLA_CONTAINER:-scylla}"
KEYSPACE="${KEYSPACE:-okx}"

RAW="$(mktemp -d)"
trap 'rm -rf "$RAW"' EXIT

# --- helpers ---------------------------------------------------------------

section() { printf '\n\n=== %s ===\n\n' "$1" | tee -a "$OUT" >/dev/null; }
line() { printf '%s\n' "$1" | tee -a "$OUT" >/dev/null; }

cql() {
  local q="$1"
  docker exec "$CONTAINER" cqlsh -e "$q" 2>&1 |
    sed 's/\r$//' |
    tee -a "$OUT" >/dev/null ||
    line "[query failed — see above]"
}

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

# Pull the full report set once — every section that needs row-level data
# works from this same file. Cuts docker-exec latency by ~5×.
cql_raw "SELECT round_id, instid, reason, earnings, change, time_left, highest, lowest, ts, strategy
           FROM $KEYSPACE.reports LIMIT 20000;" "$RAW/reports.txt"

# --- 1. headline numbers ---------------------------------------------------

section "1. headline"
cql "SELECT COUNT(*), AVG(earnings), SUM(earnings), MIN(earnings), MAX(earnings)
       FROM $KEYSPACE.reports;"

# --- 2. reason distribution ------------------------------------------------

section "2. reason distribution"
awk -F'|' '
NR > 3 && NF >= 3 {
    r = $3; gsub(/ /, "", r);
    if (r == "" || r == "reason") next;
    n[r]++; total++;
}
END {
    if (total == 0) { print "no rows"; exit }
    printf "%-16s %6s %8s\n", "reason", "n", "pct";
    printf "%-16s %6d %7.1f%%\n", "TOTAL", total, 100.0;
    for (k in n) printf "%-16s %6d %7.1f%%\n", k, n[k], 100*n[k]/total;
}' "$RAW/reports.txt" | tee -a "$OUT" >/dev/null

# --- 3. reason × earnings --------------------------------------------------
# The key diagnostic. avg_hi is "how high did this class of trades pop
# during the hold"; avg_lo is "how low did they dip". For timeouts, if
# avg_hi is meaningfully positive but avg_$ is negative, holds are giving
# back winners — a trailing-stop conversation.

section "3. earnings breakdown by exit reason"
awk -F'|' '
NR > 3 && NF >= 8 {
    for (i = 1; i <= NF; i++) gsub(/ /, "", $i);
    r = $3;
    if (r == "" || r == "reason") next;
    n[r]++; sum[r] += $4; ch[r] += $5; tl[r] += $6; hi[r] += $7; lo[r] += $8;
}
END {
    if (length(n) == 0) { print "no rows"; exit }
    printf "%-15s %6s %11s %10s %10s %10s %10s %10s\n",
           "reason", "n", "total_$", "avg_$", "avg_chg%", "avg_time_s", "avg_hi%", "avg_lo%";
    for (k in n)
        printf "%-15s %6d %11.3f %10.4f %10.4f %10.1f %10.4f %10.4f\n",
               k, n[k], sum[k], sum[k]/n[k], ch[k]/n[k], tl[k]/n[k], hi[k]/n[k], lo[k]/n[k];
}' "$RAW/reports.txt" | sort -k3 -gr | tee -a "$OUT" >/dev/null

# --- 4. per-token sequence -------------------------------------------------
# Chronological W/L pattern for tokens traded 3+ times. Reveals whether
# frequently-traded tokens were in a losing streak (persistent bad regime,
# candidate for deny list) or oscillating (regime unclear, don't deny).
#
# Legend: W = earnings > +$0.05,  L = earnings < -$0.05,  · = flat.

section "4. per-token sequence (3+ trades, worst-net first)"
awk -F'|' '
NR > 3 && NF >= 9 {
    for (i = 1; i <= NF; i++) gsub(/^ +| +$/, "", $i);
    inst = $2; ts = $9; e = $4 + 0;
    if (inst == "" || inst == "instid") next;
    sortkey = substr(ts, 1, 19);
    print inst "|" sortkey "|" e;
}' "$RAW/reports.txt" | sort -t'|' -k1,1 -k2,2 >"$RAW/timeline.txt"

{
  printf "%-14s %5s %10s  %s\n" "instid" "n" "net_$" "sequence (oldest → newest)"
  printf "%-14s %5s %10s  %s\n" "------" "-" "-----" "--------------------------"
  awk -F'|' '
    {
        inst = $1; e = $3 + 0;
        if (e > 0.05)       out[inst] = out[inst] "W";
        else if (e < -0.05) out[inst] = out[inst] "L";
        else                out[inst] = out[inst] "·";
        net[inst] += e;
        n[inst]++;
    }
    END {
        for (k in n) {
            if (n[k] >= 3) printf "%.6f|%s|%d|%s\n", net[k], k, n[k], out[k];
        }
    }' "$RAW/timeline.txt" |
    sort -t'|' -k1,1g |
    awk -F'|' '{ printf "%-14s %5d %10.3f  %s\n", $2, $3, $1, $4 }'
} | tee -a "$OUT" >/dev/null

# --- 5. loss shape ---------------------------------------------------------
# For losing trades, was the failure entry-side (straight down from entry)
# or exit-side (touched profitable territory, gave it back)?
#
# "Bad entry" = never got above +0.1% during the hold; went underwater fast
# and stayed. "Bad exit" = touched at least +0.3% then ended negative; entry
# worked, exit didn't. Fix direction is entry-side vs exit-side and they're
# different code changes.

section "5. loss shape (entry-side vs exit-side)"
awk -F'|' '
NR > 3 && NF >= 8 {
    for (i = 1; i <= NF; i++) gsub(/^ +| +$/, "", $i);
    if ($2 == "" || $2 == "instid") next;
    e = $4 + 0;
    if (e >= 0) next; # winners: see section 7.
    hi = $7 + 0;
    if (hi < 0.1)      { n_bad_entry++; sum_bad_entry += e; }
    else if (hi < 0.3) { n_middle++;    sum_middle    += e; }
    else               { n_bad_exit++;  sum_bad_exit  += e; }
    total_n++; total_sum += e;
}
END {
    if (total_n == 0) { print "no losing trades"; exit }
    printf "%-42s %5s %11s %10s\n", "loss shape", "n", "total_$", "avg_$";
    printf "%-42s %5d %11.3f %10.4f\n", "bad entry (never rose above +0.1%)",
           n_bad_entry, sum_bad_entry, n_bad_entry ? sum_bad_entry/n_bad_entry : 0;
    printf "%-42s %5d %11.3f %10.4f\n", "middle (peaked +0.1% to +0.3%)",
           n_middle, sum_middle, n_middle ? sum_middle/n_middle : 0;
    printf "%-42s %5d %11.3f %10.4f\n", "bad exit (peaked +0.3% or more)",
           n_bad_exit, sum_bad_exit, n_bad_exit ? sum_bad_exit/n_bad_exit : 0;
    printf "%-42s %5d %11.3f %10.4f\n", "TOTAL LOSSES",
           total_n, total_sum, total_sum/total_n;
    printf "\ninterpretation:\n";
    printf "  bad_entry dominant → entries catch tops; tighten entry filter.\n";
    printf "  bad_exit dominant  → entries fine, exits leave money on table;\n";
    printf "                       tighten sell_floor or add a trailing stop.\n";
}' "$RAW/reports.txt" | tee -a "$OUT" >/dev/null

# --- 6. timeout dissection -------------------------------------------------
# Timeouts are the biggest reason category and where most $ leak from.
# Split by whether they ever brushed a winning level. Complements section 5:
# if timeouts overwhelmingly "never went positive", entries are the problem;
# if a chunk went positive then reversed, timeouts are giving back real gains.

section "6. timeout dissection"
awk -F'|' '
NR > 3 && NF >= 8 {
    for (i = 1; i <= NF; i++) gsub(/^ +| +$/, "", $i);
    if ($3 != "timeout") next;
    hi = $7 + 0; e = $4 + 0;
    if (hi <= 0)         { n1++; sum1 += e; hi1 += hi; }
    else if (hi < 0.3)   { n2++; sum2 += e; hi2 += hi; }
    else if (hi < 0.6)   { n3++; sum3 += e; hi3 += hi; }
    else                 { n4++; sum4 += e; hi4 += hi; }
    total++;
}
END {
    if (total == 0) { print "no timeouts"; exit }
    printf "%-34s %5s %11s %10s %10s\n", "peak reached during hold", "n", "total_$", "avg_$", "avg_hi%";
    printf "%-34s %5d %11.3f %10.4f %10.4f\n", "never went positive",
           n1, sum1, n1 ? sum1/n1 : 0, n1 ? hi1/n1 : 0;
    printf "%-34s %5d %11.3f %10.4f %10.4f\n", "peaked <+0.3% (noise)",
           n2, sum2, n2 ? sum2/n2 : 0, n2 ? hi2/n2 : 0;
    printf "%-34s %5d %11.3f %10.4f %10.4f\n", "peaked +0.3% to +0.6%",
           n3, sum3, n3 ? sum3/n3 : 0, n3 ? hi3/n3 : 0;
    printf "%-34s %5d %11.3f %10.4f %10.4f\n", "peaked +0.6% or more",
           n4, sum4, n4 ? sum4/n4 : 0, n4 ? hi4/n4 : 0;
    printf "%-34s %5d\n", "TOTAL timeouts", total;
    printf "\ninterpretation:\n";
    printf "  Rows 3+4 with negative avg_$ = timeouts that reached sell_floor\n";
    printf "  territory and reversed. Those would have been floor_reached wins\n";
    printf "  with a tighter sell_floor or a trailing stop.\n";
}' "$RAW/reports.txt" | tee -a "$OUT" >/dev/null

# --- 7. winning trades ----------------------------------------------------

section "7. top 15 winning trades"
{
  printf "%-16s %-14s %11s %8s %10s %8s %8s\n" \
    "instid" "reason" "earnings" "change" "time_left" "highest" "lowest"
  printf "%-16s %-14s %11s %8s %10s %8s %8s\n" \
    "------" "------" "--------" "------" "---------" "-------" "------"
  awk -F'|' '
    NR > 3 && NF >= 8 {
        for (i = 1; i <= NF; i++) gsub(/^ +| +$/, "", $i);
        e = $4 + 0;
        if (e <= 0.05) next;
        printf "%-16s %-14s %11.4f %8.2f %10d %8.2f %8.2f\n",
               $2, $3, e, $5+0, $6+0, $7+0, $8+0;
    }' "$RAW/reports.txt" | sort -k3 -gr | head -15
} | tee -a "$OUT" >/dev/null

# --- 8. strategy configs present in the reports table ---------------------

section "8. strategy configs present in the reports table"
awk -F'|' '
NR > 3 && NF >= 10 {
    s = $10; gsub(/^ +| +$/, "", s);
    if (s == "" || s == "strategy") next;
    n[s]++;
}
END {
    for (k in n) printf "%s  reports=%d\n", k, n[k];
}' "$RAW/reports.txt" | sort -t= -k2 -rn | tee -a "$OUT" >/dev/null

section "8b. active strategies table (config that produced each hash)"
cql "SELECT hash, timeframe, timeout, cooldown, min_change, min_vol,
            min_rising_candles, sell_floor, stoploss, cashout, min_change_last_candle
       FROM $KEYSPACE.strategies LIMIT 20;"

# --- footer ---------------------------------------------------------------

section "done"
line "wrote: $OUT"
printf '\nreport ready: %s\n' "$OUT"

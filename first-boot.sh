#!/usr/bin/env bash
#
# provision.sh — bring up infra and set up topics + schema.
# Idempotent: safe to re-run.

set -euo pipefail

# ---- config ----------------------------------------------------------------
SCYLLA_CONTAINER="scylla"
REDPANDA_CONTAINER="redpanda"
KEYSPACE="okx"
MIGRATION_PATH="/tmp/migration.cql" # path inside the scylla container
TOPICS=(candle1m tickers trades)
PARTITIONS=10
REPLICAS=1
RETENTION_MS=43200000 # 12h
SCYLLA_TIMEOUT=180    # seconds
REDPANDA_TIMEOUT=60   # seconds

# ---- helpers ---------------------------------------------------------------
c_green="\033[32m"
c_yellow="\033[33m"
c_red="\033[31m"
c_reset="\033[0m"
log() { printf "${c_green}==>${c_reset} %s\n" "$*"; }
warn() { printf "${c_yellow}!!${c_reset}  %s\n" "$*" >&2; }
die() {
  printf "${c_red}xx${c_reset}  %s\n" "$*" >&2
  exit 1
}

command -v docker >/dev/null || die "docker not found in PATH"

# On macOS/Docker Desktop, aio-max-nr is often too low. Bump it live in the VM.
# Ignored on Linux where the value is already high enough.
bump_aio() {
  if [[ "$(uname -s)" == "Darwin" ]]; then
    log "Bumping fs.aio-max-nr inside Docker Desktop VM"
    docker run --privileged --rm alpine sh -c \
      "sysctl -w fs.aio-max-nr=1048576" >/dev/null 2>&1 ||
      warn "Could not raise aio-max-nr — continuing (may fail on redpanda/scylla start)"
  fi
}

wait_for_scylla() {
  log "Waiting for Scylla to be UN (up to ${SCYLLA_TIMEOUT}s)"
  local deadline=$((SECONDS + SCYLLA_TIMEOUT))
  while ((SECONDS < deadline)); do
    if docker exec "$SCYLLA_CONTAINER" nodetool status 2>/dev/null | grep -q '^UN'; then
      log "Scylla is UN"
      return 0
    fi
    sleep 3
  done
  die "Scylla did not reach UN within ${SCYLLA_TIMEOUT}s. Check: docker compose logs scylla"
}

wait_for_redpanda() {
  log "Waiting for Redpanda cluster health (up to ${REDPANDA_TIMEOUT}s)"
  local deadline=$((SECONDS + REDPANDA_TIMEOUT))
  while ((SECONDS < deadline)); do
    if docker exec "$REDPANDA_CONTAINER" rpk cluster health 2>/dev/null | grep -q 'Healthy:.*true'; then
      log "Redpanda is healthy"
      return 0
    fi
    sleep 2
  done
  die "Redpanda did not become healthy within ${REDPANDA_TIMEOUT}s. Check: docker compose logs redpanda"
}

run_migrations() {
  log "Running CQL migrations from ${MIGRATION_PATH}"
  docker exec "$SCYLLA_CONTAINER" cqlsh -f "$MIGRATION_PATH" ||
    die "Migration failed"

  log "Verifying keyspace '${KEYSPACE}' tables"
  docker exec "$SCYLLA_CONTAINER" cqlsh -e "USE ${KEYSPACE}; DESCRIBE TABLES;"
}

create_topics() {
  log "Creating Redpanda topics: ${TOPICS[*]}"
  # rpk topic create is idempotent — errors out on existing topics but we
  # tolerate that so re-runs are clean.
  docker exec "$REDPANDA_CONTAINER" rpk topic create "${TOPICS[@]}" \
    --partitions "$PARTITIONS" --replicas "$REPLICAS" 2>&1 |
    grep -v 'TOPIC_ALREADY_EXISTS' || true

  log "Setting retention.ms=${RETENTION_MS} on topics"
  docker exec "$REDPANDA_CONTAINER" rpk topic alter-config "${TOPICS[@]}" \
    --set "retention.ms=${RETENTION_MS}"

  log "Topics:"
  docker exec "$REDPANDA_CONTAINER" rpk topic list
}

# ---- main ------------------------------------------------------------------
log "Starting containers"
bump_aio
docker compose up -d

wait_for_scylla
wait_for_redpanda
run_migrations
create_topics

cat <<EOF

$(printf "${c_green}==>${c_reset}") All set. Endpoints:
    ScyllaDB:         127.0.0.1:9042
    Redpanda:         127.0.0.1:9092
    Redpanda console: http://localhost:8080/topics

Next:
    cargo run --bin producer
    cargo run --bin consumer
    cargo run --bin scheduler
EOF

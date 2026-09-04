#!/bin/bash
docker exec scylla cqlsh -e "
  TRUNCATE okx.candle1m;
  TRUNCATE okx.tickers;
  TRUNCATE okx.reports;
  TRUNCATE okx.strategies;
"

docker exec redpanda rpk topic delete candle1m tickers reports
docker exec redpanda rpk topic create candle1m -p 10
docker exec redpanda rpk topic create tickers -p 10
docker exec redpanda rpk topic create reports -p 10

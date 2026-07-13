USE okx;

DROP TABLE IF EXISTS candle1m;

CREATE TABLE IF NOT EXISTS candle1m (
  instid text,
  open double,
  high double,
  low double,
  close double,
  volume double,
  change double,
  range double,
  ts timestamp,
  primary key (instid, ts))
WITH default_time_to_live = 86400
AND CLUSTERING ORDER BY (ts desc)
AND compaction = {'class': 'TimeWindowCompactionStrategy', 'compaction_window_size': 3};

DROP TABLE IF EXISTS reports;
CREATE TABLE IF NOT EXISTS reports (
  round_id bigint,
  instid text,
  buy_price double,
  sell_price double,
  earnings double,
  fees double,
  reason text,
  highest double,
  highest_elapsed bigint,
  lowest double,
  lowest_elapsed bigint,
  change double,
  time_left bigint,
  strategy text,
  ts timestamp,
  primary key ((round_id, instid), ts))
WITH CLUSTERING ORDER BY (ts desc);

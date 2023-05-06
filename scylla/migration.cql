CREATE KEYSPACE IF NOT EXISTS okx WITH REPLICATION = { 'class' : 'SimpleStrategy','replication_factor' : 1 };
USE okx;

CREATE TABLE IF NOT EXISTS trades (
  instid text,
  sz double,
  tradeid int,
  px double,
  side text,
  ts timestamp,
  primary key (instid, ts, tradeid))
WITH default_time_to_live = 2592000
AND CLUSTERING ORDER BY (ts desc)
AND compaction = {'class': 'TimeWindowCompactionStrategy', 'compaction_window_size': 3};

CREATE TABLE IF NOT EXISTS tickers (
  instid text,
  last double,
  bidpx double,
  vol24h double,
  volccy24h double,
  askpx double,
  asksz double,
  bidsz double,
  lastsz double,
  high24h double,
  low24h double,
  open24h double,
  sodutc0 double,
  sodutc8 double,
  ts timestamp,
  primary key (instid, ts, last))
WITH default_time_to_live = 2592000
AND CLUSTERING ORDER BY (ts desc)
AND compaction = {'class': 'TimeWindowCompactionStrategy', 'compaction_window_size': 3};

CREATE TABLE IF NOT EXISTS candle1m (
  instid text,
  open double,
  high double,
  low double,
  close double,
  volume double,
  change float,
  range float,
  ts timestamp,
  primary key (instid, ts))
WITH default_time_to_live = 2592000
AND CLUSTERING ORDER BY (ts desc)
AND compaction = {'class': 'TimeWindowCompactionStrategy', 'compaction_window_size': 3};

CREATE TABLE IF NOT EXISTS reports (
  round_id bigint,
  instid text,
  buy_price double,
  sell_price double,
  earnings double,
  reason text,
  highest float,
  highest_elapsed bigint,
  lowest float,
  lowest_elapsed bigint,
  change float,
  time_left bigint,
  strategy text,
  ts timestamp,
  primary key ((round_id, instid), ts))
WITH CLUSTERING ORDER BY (ts desc);

CREATE TABLE IF NOT EXISTS orders (
  ord_id text,
  inst_id text,
  td_mode text,
  cl_ord_id text,
  side text,
  ord_type text,
  px text,
  sz text,
  strategy text,
  ts timestamp,
  primary key ((ord_id, inst_id), ts))
WITH CLUSTERING ORDER BY (ts desc);

CREATE TABLE IF NOT EXISTS strategies (
    hash text,
    top int,
    portfolio_size int,
    timeframe int,
    cooldown int,
    timeout int,
    min_vol double,
    min_change float,
    min_change_last_candle float,
    min_deviation double,
    deny_list list<text>,
    cashout float,
    quickstart boolean,
    stoploss double,
    avoid_after_stoploss boolean,
    sell_floor double,
primary key (hash));

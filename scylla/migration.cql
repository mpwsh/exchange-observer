CREATE KEYSPACE IF NOT EXISTS okx WITH REPLICATION = { 'class' : 'SimpleStrategy','replication_factor' : 1 };
USE okx;

CREATE TABLE IF NOT EXISTS trades (
  instid text,
  sz float,
  tradeid int,
  px float,
  side text,
  ts timestamp,
  primary key (instid, ts, tradeid))
WITH default_time_to_live = 2592000
AND CLUSTERING ORDER BY (ts desc)
AND compaction = {'class': 'TimeWindowCompactionStrategy', 'compaction_window_size': 3};


CREATE TABLE IF NOT EXISTS tickers (
  instid text,
  last float,
  bidpx float,
  vol24h float,
  volccy24h float,
  askpx float,
  asksz float,
  bidsz float,
  lastsz float,
  high24h float,
  low24h float,
  open24h float,
  sodutc0 float,
  sodutc8 float,
  ts timestamp,
  primary key (instid, ts, last))
WITH default_time_to_live = 2592000
AND CLUSTERING ORDER BY (ts desc)
AND compaction = {'class': 'TimeWindowCompactionStrategy', 'compaction_window_size': 3};

CREATE TABLE IF NOT EXISTS candle1m (
  instid text,
  open float,
  high float,
  low float,
  close float,
  volume float,
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
  buy_price float,
  sell_price float,
  earnings float,
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

CREATE TABLE IF NOT EXISTS strategies (
    hash text,
    top int,
    portfolio_size int,
    timeframe int,
    cooldown int,
    timeout int,
    min_vol float,
    min_change float,
    min_change_last_candle float,
    min_deviation float,
    deny_list list<text>,
    cashout float,
    quickstart boolean,
    stoploss float,
    avoid_after_stoploss boolean,
    sell_floor float,
primary key (hash));

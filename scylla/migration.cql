-- migration.cql
--
-- Safe to run on a fresh keyspace or on a live one. Same file either way.
--
-- TTLs, and why they differ:
--   tickers    1h   -- only the newest row is ever read. This is the table that costs you.
--   trades     1h   -- nothing subscribes to it; nothing writes to it.
--   books      1h   -- likewise.
--   candle1m   12h  -- the scheduler needs 20 min, but `trade_replay` charts every
--                      trade from these rows and you read the reports hours later.
--   reports    none -- your only history. NEVER put a TTL on this.
--   orders     none
--   strategies none -- maps a config hash to what it meant.
--
-- What it destroys: candle1m, tickers, trades, books, order_entry, strategies.
--   The four market-data tables all carry a TTL and refill themselves.
--   `strategies` is a registry the scheduler rewrites on every boot (see
--   App::save_strategy). It has to be recreated rather than altered -- more on
--   that below.
--
-- What it preserves: reports, orders.
--   Neither has a TTL. `reports` is the only record of what the bot actually
--   did. The previous version of this file opened with `DROP TABLE IF EXISTS
--   reports;`, so every re-run wiped your entire trading history. That is gone.
--
-- BEFORE YOU RUN IT, if you want to keep the old hash -> config mapping:
--
--     cqlsh -e "COPY okx.strategies TO 'strategies_backup.csv' WITH HEADER=true"
--
--   You are about to invalidate every hash anyway -- `max_spread_bps` is a new
--   field, and `get_hash()` covers the whole struct -- so old rows only matter
--   for decoding the `strategy` column on historical reports.

CREATE KEYSPACE IF NOT EXISTS okx WITH REPLICATION = { 'class' : 'SimpleStrategy','replication_factor' : 1 };
USE okx;

-- ===========================================================================
-- Market data. All 24h TTL, all disposable.
-- ===========================================================================

-- candle1m --------------------------------------------------------------------
-- + confirm: OKX element 8, false while the bar is still being built. The parser
--   ignored it, so partial minutes were stored indistinguishably from closed ones.
--
--   The column stays null unless the producer writes it, via
--   `Candlestick::to_row_with_confirm` (10 bind markers, not 9). The alternative
--   -- and the one I'd pick -- is to not store it and just never write an
--   unconfirmed bar:
--
--       if !candle.confirm { continue; }
--
--   candle1m then only ever holds closed minutes, which is the cleaner invariant:
--   the scheduler already rebuilds the current minute from `tickers`. In that case
--   drop the column from the CREATE below.
--
-- Existing rows have `change` and `range` quantized to 0.01% by the `.round()`
-- that used to live in `get_change()` / `get_range()`. The TTL washes that out in
-- a day -- just don't tune min_dip / min_bounce against data older than the deploy.

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
  confirm boolean,
  ts timestamp,
  primary key (instid, ts))
-- 12h, down from 24h.
--
-- The scheduler only ever reads the last `timeframe` minutes. The reason this is
-- not an hour is `analysis/sections/trade_replay.py`: every candle chart in the
-- report is drawn from these rows, around the timestamp of the trade. Cut this to
-- 1h and you can only ever chart trades from the last hour -- which is the single
-- most useful diagnostic you have, and you read those reports hours after the fact.
--
-- One row per instrument per minute. Next to `tickers` this table is free; it is
-- not what is costing you.
WITH default_time_to_live = 43200
AND CLUSTERING ORDER BY (ts desc)
AND compaction = {'class': 'TimeWindowCompactionStrategy',
                  'compaction_window_unit': 'HOURS',
                  'compaction_window_size': 1};

-- tickers ---------------------------------------------------------------------
-- `last` removed from the primary key.
--
-- It was a clustering column -- primary key (instid, ts, last) -- with no order
-- given, so it defaulted to ASC while `ts` was DESC. That means
--
--     SELECT last, askpx, bidpx FROM tickers WHERE instid=? LIMIT 1
--
-- returned, among ticks sharing the newest millisecond, the one with the *lowest*
-- price. Small, but a systematic downward bias on the number every order is priced
-- from -- and it stacks on top of everything else that was pushing entries the
-- wrong way.
--
-- Dropping it means same-millisecond ticks upsert (last write wins) instead of
-- becoming separate rows. You lose the occasional duplicate; you only ever read
-- the newest one, and now you read it without a bias. Keeping `last` with
-- CLUSTERING ORDER BY (ts desc, last desc) would just flip the bias upward.

DROP TABLE IF EXISTS tickers;
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
  primary key (instid, ts))
-- 1 hour. Nothing reads a ticker older than the newest one:
--
--     SELECT last, askpx, asksz, bidpx, bidsz FROM tickers WHERE instid=? LIMIT 1
--
-- That is the only query against this table, and `LIMIT 1` on a `ts desc` clustering
-- order means row zero. An hour is already pure margin.
--
-- This is also the table that costs you. At ~250 instruments x ~10 updates/sec it is
-- the overwhelming majority of the ~700 msg/s the consumer handles; candle1m is one
-- row per instrument per minute.
--
-- The compaction window was 3 HOURS -- longer than the TTL now is, which means TWCS
-- could never drop a whole SSTable cleanly and just kept compacting. Rule of thumb is
-- ~20-30 windows across the TTL, so 3-minute windows over 1 hour. This is the likeliest
-- explanation for a P99.9 insert latency of 3,800ms that climbed steadily the longer
-- the process ran, while avg latency sat at 0-1ms. A rotten tail, not a slow database.
--
-- NOTE: `default_time_to_live` applies to NEW writes only. Existing rows keep whatever
-- TTL they were written with.
WITH default_time_to_live = 3600
AND CLUSTERING ORDER BY (ts desc)
AND compaction = {'class': 'TimeWindowCompactionStrategy',
                  'compaction_window_unit': 'MINUTES',
                  'compaction_window_size': 3};

-- trades ----------------------------------------------------------------------
-- tradeid int -> bigint. OKX trade ids pass 2^31 on active instruments, and the
-- field is parsed from a string -- so a too-large id is a serde *error* and the
-- whole trade message is dropped on ingest, not merely the id.
--
-- `tradeid` is a clustering column, so this is a recreate, not an alter. Nothing
-- in the scheduler queries this table (it reads tickers, candle1m, reports,
-- orders, strategies); confirm nothing outside the workspace does either.

DROP TABLE IF EXISTS trades;
CREATE TABLE IF NOT EXISTS trades (
  instid text,
  sz double,
  tradeid bigint,
  px double,
  side text,
  ts timestamp,
  primary key (instid, ts, tradeid))
-- 1 hour. The config subscribes to `candle1m` and `tickers` only, so nothing writes
-- to this table at all -- but the consumer prepares a statement against it at boot and
-- will refuse to start if it is missing.
WITH default_time_to_live = 3600
AND CLUSTERING ORDER BY (ts desc)
AND compaction = {'class': 'TimeWindowCompactionStrategy',
                  'compaction_window_unit': 'MINUTES',
                  'compaction_window_size': 3};

-- books -----------------------------------------------------------------------
-- Look before you leap:  SELECT instid, ts, asks, bids FROM books LIMIT 20;
--
-- I expect asks/bids to come back empty. `order_entry.quantity` was `int`; OKX
-- sends spot sizes as decimal strings ("8.5"); the parse failed; and
-- `de_order_entries` silently filter_map'd every fractional level away. On spot
-- that is nearly every level. There is nothing in this table to migrate.
--
--   price float -> double : f32 cannot resolve the tick on a token at 0.0000123
--   quantity int -> double: the bug
--   seq_id, prev_seq_id int -> bigint : `as i32` wrapped OKX's sequence ids into
--                                       plausible-looking lies
--
-- `seq_id` is a clustering column and Scylla has no ALTER TYPE ... ALTER <field>,
-- so both the table and the UDT have to go. Table first -- it depends on the type.

DROP TABLE IF EXISTS books;
DROP TYPE IF EXISTS order_entry;

CREATE TYPE IF NOT EXISTS order_entry (
    price double,
    quantity double,
    deprecated int,
    num_orders int
);

CREATE TABLE IF NOT EXISTS books (
  instid text,
  asks list<frozen<order_entry>>,
  bids list<frozen<order_entry>>,
  checksum int,
  prev_seq_id bigint,
  seq_id bigint,
  ts timestamp,
  primary key (instid, ts, seq_id))
-- 1 hour. As with `trades`: the config subscribes to candle1m and tickers only, so
-- nothing writes here -- but the consumer prepares a statement against it at boot and
-- will refuse to start if the table is missing.
WITH default_time_to_live = 3600
AND CLUSTERING ORDER BY (ts desc)
AND compaction = {'class': 'TimeWindowCompactionStrategy',
                  'compaction_window_unit': 'MINUTES',
                  'compaction_window_size': 3};

-- ===========================================================================
-- strategies. Recreated, not altered.
-- ===========================================================================
-- + max_spread_bps.
--
-- This one is load-bearing: `App::save_strategy` writes the whole struct with
-- `INSERT INTO okx.strategies JSON '...'`, and Scylla rejects a JSON insert
-- carrying a key that is not a column. Add a field to
-- `exchange_observer::Strategy` without adding it here and the scheduler dies on
-- startup, before the first cycle, every time.
--
-- `CREATE TABLE IF NOT EXISTS` is a no-op against your existing table, so it
-- would not add the column -- hence the DROP. If you would rather keep the old
-- hash -> config rows, skip the DROP and run this single statement instead:
--
--     ALTER TABLE strategies ADD max_spread_bps float;
--
-- (that is the *only* line in this file that has to differ between a fresh
-- keyspace and yours, which is why the whole thing is one file and not two.)

DROP TABLE IF EXISTS strategies;
CREATE TABLE IF NOT EXISTS strategies (
    hash text,
    strategy_type text,
    order_type text,
    top int,
    portfolio_size int,
    timeframe int,
    cooldown int,
    timeout int,
    min_vol double,
    max_spread_bps float,
    min_change float,
    min_change_last_candle float,
    min_deviation double,
    max_deviation double,
    deny_list list<text>,
    cashout float,
    quickstart boolean,
    stoploss double,
    avoid_after_stoploss boolean,
    sell_floor double,
    min_rising_candles int,
    dip_window int,
    bounce_window int,
    min_dip float,
    min_dip_std float,
    min_bounce float,
    min_bounce_std float,
    avoid_falling_knives boolean,
primary key (hash));

-- `min_dip_std` and `min_bounce_std` are new. ON AN EXISTING KEYSPACE the CREATE
-- above is a no-op and will NOT add them:
--
--   ALTER TABLE strategies ADD min_dip_std float;
--   ALTER TABLE strategies ADD min_bounce_std float;
--
-- `App::save_strategy` does INSERT ... JSON and Scylla rejects a JSON insert
-- carrying a key that is not a column, so without these the scheduler dies at boot.

-- ===========================================================================
-- Never dropped. This is your history.
-- ===========================================================================

-- The four entry-condition columns are new. `reports` recorded what happened and
-- never why we were there -- so "do deeper dips produce better trades?", the
-- question that decides `min_dip`, could not be answered from the data at all,
-- and the threshold got set by argument instead of evidence.
--
-- Note these are the values that *cleared* the thresholds, not the thresholds
-- themselves. Storing `min_dip` would tell you nothing: every entry cleared it by
-- definition.
--
-- ON AN EXISTING KEYSPACE this CREATE is a no-op and will NOT add them. Run these
-- four instead (additive, safe, no data loss):
--
--   ALTER TABLE reports ADD dip double;
--   ALTER TABLE reports ADD bounce double;
--   ALTER TABLE reports ADD std_deviation double;
--   ALTER TABLE reports ADD spread_bps double;
--
-- `Report::save` does INSERT ... JSON, and Scylla rejects a JSON insert carrying a
-- key that is not a column -- so without these the scheduler will fail on the
-- first closed trade, not at boot. Worse than a boot failure: it runs, it trades,
-- and it dies the moment it tries to record the result.

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
  dip double,
  bounce double,
  std_deviation double,
  spread_bps double,
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

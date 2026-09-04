pub use std::{error::Error, str::FromStr, sync::Arc, time};

pub use anyhow::Result;
pub use chrono::{
    DateTime, Duration, NaiveDateTime, SecondsFormat, TimeDelta, TimeZone, Timelike, Utc,
};
// `Strategy` is now the engine's strategy *trait*; the threshold config bag
// from `lib` (formerly imported here as `Strategy`) is `StrategyConfig`.
//
// `ReversionStrategy` was added here: it existed in `engine` and was exported
// from `engine::lib`, but the scheduler never imported it and `main` hardcoded
// `ThresholdStrategy`, so it had never once run outside its own unit tests.
pub use engine::{
    Candle, Clock, Context, EnterDecision, EntrySignal, ExitDecision, LiveClock, PortfolioView,
    PositionView, ReversionStrategy, Strategy, StrategyConfig, ThresholdStrategy, TokenView,
};
pub use exchange_observer::{AppConfig, Authentication, Exchange, OffsetDateTime, Pushover};
pub use scylla::{
    client::{session::Session, session_builder::SessionBuilder, Compression},
    response::query_result::QueryResult,
    DeserializeRow,
};
pub use serde_derive::{Deserialize, Serialize};
pub use serde_with::{DurationMilliSeconds, DurationSeconds};
pub use uuid::Uuid;

pub use crate::{
    app::App,
    models::{
        account::{Account, Balance},
        report::Report,
        token::{self, Candlestick, Status, Token, TopOfBook},
        trade::{self, ExitReason, Order, Side, State as OrderState},
    },
    okx::*,
    utils::*,
    BALANCE_ENDPOINT, BASE_URL, ORDERS_ENDPOINT,
};

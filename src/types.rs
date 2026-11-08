//! Orders, events and the rules that do not depend on the book's state.
//!
//! The numeric values of the enums match matching-engine's Go `book` package, so events
//! from both implementations can be hashed and compared (see `tests/conformance.rs`).

/// Identifies an order. The caller assigns ids; 0 is never valid.
pub type OrderId = u64;

/// Side of an order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Side {
    /// Buy.
    Buy = 1,
    /// Sell.
    Sell = 2,
}

impl Side {
    /// The side an order of this side trades against.
    #[must_use]
    pub const fn opposite(self) -> Side {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }
}

/// Type of an order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum OrderType {
    /// Trades at its price or better.
    Limit = 1,
    /// Trades at any price; never rests on the book.
    Market = 2,
}

/// What happens to the part of an order that does not trade at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TimeInForce {
    /// Good till canceled: the remainder rests on the book.
    Gtc = 1,
    /// Immediate or cancel: the remainder is canceled.
    Ioc = 2,
    /// Fill or kill: the order fills completely at once, or is rejected.
    Fok = 3,
}

/// Self-trade prevention: two orders with the same non-zero owner never trade with each
/// other; the mode of the incoming (taker) order decides which one is canceled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Stp {
    /// Cancel the rest of the incoming order (the default).
    #[default]
    CancelNewest = 0,
    /// Cancel the resting order and keep matching.
    CancelOldest = 1,
    /// Cancel both.
    CancelBoth = 2,
}

/// A new order for [`crate::Book::submit`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Order {
    /// Unique, non-zero.
    pub id: OrderId,
    /// Account; 0 means anonymous, and self-trade prevention does not apply.
    pub owner: u64,
    /// Buy or sell.
    pub side: Side,
    /// Limit or market.
    pub kind: OrderType,
    /// Time in force.
    pub tif: TimeInForce,
    /// Limit price in ticks; ignored for market orders.
    pub price: i64,
    /// Quantity in lots.
    pub qty: i64,
    /// GTC limit only: reject the order instead of letting it take liquidity.
    pub post_only: bool,
    /// Self-trade prevention mode.
    pub stp: Stp,
}

impl Order {
    /// A GTC limit order with no owner.
    pub const fn limit(id: OrderId, side: Side, price: i64, qty: i64) -> Order {
        Order {
            id,
            owner: 0,
            side,
            kind: OrderType::Limit,
            tif: TimeInForce::Gtc,
            price,
            qty,
            post_only: false,
            stp: Stp::CancelNewest,
        }
    }

    /// An IOC market order with no owner.
    pub const fn market(id: OrderId, side: Side, qty: i64) -> Order {
        Order {
            id,
            owner: 0,
            side,
            kind: OrderType::Market,
            tif: TimeInForce::Ioc,
            price: 0,
            qty,
            post_only: false,
            stp: Stp::CancelNewest,
        }
    }

    /// The same order with an owner.
    #[must_use]
    pub const fn owned_by(mut self, owner: u64) -> Order {
        self.owner = owner;
        self
    }

    /// The same order with another time in force.
    #[must_use]
    pub const fn with_tif(mut self, tif: TimeInForce) -> Order {
        self.tif = tif;
        self
    }
}

/// A resting order, as [`crate::Book::lookup`] and [`crate::Book::orders`] report it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrderInfo {
    /// Order id.
    pub id: OrderId,
    /// Account.
    pub owner: u64,
    /// Side.
    pub side: Side,
    /// Price in ticks.
    pub price: i64,
    /// Open (leaves) quantity.
    pub qty: i64,
    /// Post-only flag.
    pub post_only: bool,
    /// Self-trade prevention mode.
    pub stp: Stp,
}

/// The aggregated quantity at one price.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PriceLevel {
    /// Price in ticks.
    pub price: i64,
    /// Total open quantity.
    pub qty: i64,
    /// Number of orders.
    pub orders: u32,
}

/// The new total quantity at a price after an operation; 0 means the level is gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LevelChange {
    /// Side of the level.
    pub side: Side,
    /// Price in ticks.
    pub price: i64,
    /// New total quantity.
    pub qty: i64,
}

/// The kind of an [`Event`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum EventKind {
    /// A new order passed validation.
    Accepted = 1,
    /// A request was refused; nothing changed.
    Rejected = 2,
    /// The incoming order traded with a resting one.
    Trade = 3,
    /// The remainder of the order now rests on the book.
    Rested = 4,
    /// An order, or its remainder, was canceled.
    Canceled = 5,
    /// A resting order got a new price or quantity.
    Modified = 6,
}

/// Why a request was rejected or an order canceled.
///
/// `InvalidSide`, `InvalidType` and `InvalidStp` cannot happen here: the Rust types make
/// those states unrepresentable. They are kept so the numbering matches the Go version.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
#[allow(missing_docs)]
pub enum Reason {
    #[default]
    None = 0,
    InvalidId,
    InvalidSide,
    InvalidType,
    /// Includes market GTC orders and post-only orders that are not GTC limit.
    InvalidTif,
    InvalidStp,
    InvalidPrice,
    InvalidQty,
    DuplicateId,
    UnknownOrder,
    PostOnlyWouldCross,
    FokNotFilled,
    /// The remainder of an IOC or market order found nothing more to trade with.
    NoLiquidity,
    SelfTrade,
    CanceledByUser,
}

/// One thing that happened during a submit, cancel or modify.
///
/// For a trade, `order`/`side`/`leaves`/`owner` describe the incoming (taker) order and
/// `maker`/`maker_leaves`/`maker_owner` the resting one; `price` is the maker's price.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    /// What happened.
    pub kind: EventKind,
    /// Rejected and Canceled only.
    pub reason: Reason,
    /// The order the event is about; the taker, for trades.
    pub order: OrderId,
    /// `None` when the order was unknown.
    pub side: Option<Side>,
    /// Limit, trade, resting or new price; 0 for market orders.
    pub price: i64,
    /// Order, traded, rested, canceled or new quantity.
    pub qty: i64,
    /// Open quantity of `order` after the event.
    pub leaves: i64,
    /// Trades: the resting order.
    pub maker: OrderId,
    /// Trades: open quantity of the resting order after the trade.
    pub maker_leaves: i64,
    /// Trades: the taker's account.
    pub owner: u64,
    /// Trades: the maker's account.
    pub maker_owner: u64,
}

impl Event {
    /// An event of this kind with every other field empty; fill in the rest with struct
    /// update syntax: `Event { order: 7, ..Event::of(EventKind::Accepted) }`.
    pub const fn of(kind: EventKind) -> Event {
        Event {
            kind,
            reason: Reason::None,
            order: 0,
            side: None,
            price: 0,
            qty: 0,
            leaves: 0,
            maker: 0,
            maker_leaves: 0,
            owner: 0,
            maker_owner: 0,
        }
    }

    /// The fields as `u64`, in the order the cross-language conformance hash uses.
    pub fn fields(&self) -> [u64; 11] {
        [
            self.kind as u64,
            self.reason as u64,
            self.order,
            self.side.map_or(0, |s| s as u64),
            self.price as u64,
            self.qty as u64,
            self.leaves as u64,
            self.maker,
            self.maker_leaves as u64,
            self.owner,
            self.maker_owner,
        ]
    }
}

/// Checks an order on its own, without looking at the book.
pub fn validate(o: &Order) -> Reason {
    if o.id == 0 {
        Reason::InvalidId
    } else if (o.kind == OrderType::Market && o.tif == TimeInForce::Gtc)
        || (o.post_only && (o.kind != OrderType::Limit || o.tif != TimeInForce::Gtc))
    {
        Reason::InvalidTif
    } else if o.kind == OrderType::Limit && o.price <= 0 {
        Reason::InvalidPrice
    } else if o.qty <= 0 {
        Reason::InvalidQty
    } else {
        Reason::None
    }
}

/// Whether an order of side `side` with limit price `limit` may trade at `price`.
pub const fn acceptable(side: Side, limit: i64, price: i64) -> bool {
    match side {
        Side::Buy => price <= limit,
        Side::Sell => price >= limit,
    }
}

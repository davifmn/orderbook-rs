//! A limit order book with price-time priority: limit and market orders; GTC, IOC and
//! FOK; post-only; cancel and modify (a quantity reduction keeps queue priority); and
//! self-trade prevention in three modes.
//!
//! It is a port of the book of [matching-engine](https://github.com/davifmn/matching-engine)
//! (Go), and is checked against it: replaying the same recorded flow must give the same
//! events and the same final digest (`tests/conformance.rs`).
//!
//! ```
//! use orderbook::{Book, EventKind, Order, Side};
//!
//! let mut book = Book::new();
//! book.submit(Order::limit(1, Side::Sell, 3415, 5));
//! let events = book.submit(Order::limit(2, Side::Buy, 3420, 3));
//! assert_eq!(events[1].kind, EventKind::Trade);
//! assert_eq!(events[1].price, 3415); // the resting order's price
//! assert_eq!(book.best_level(Side::Sell).unwrap().qty, 2);
//! ```
//!
//! Prices are integer ticks and quantities integer lots: no floating point.

#![forbid(unsafe_code)]

mod book;
pub mod flow;
pub mod hash;
pub mod reference;
mod types;

pub use book::Book;
pub use types::{
    Event, EventKind, LevelChange, Order, OrderId, OrderInfo, OrderType, PriceLevel, Reason, Side,
    Stp, TimeInForce, acceptable, validate,
};

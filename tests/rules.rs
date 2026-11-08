//! The matching rules, one by one.

use orderbook::{
    Book, Event, EventKind, LevelChange, Order, OrderType, PriceLevel, Reason, Side, Stp,
    TimeInForce,
};

fn accepted(o: &Order) -> Event {
    let price = if o.kind == OrderType::Market {
        0
    } else {
        o.price
    };
    Event {
        order: o.id,
        side: Some(o.side),
        price,
        qty: o.qty,
        leaves: o.qty,
        ..Event::of(EventKind::Accepted)
    }
}

fn rested(id: u64, side: Side, price: i64, qty: i64) -> Event {
    Event {
        order: id,
        side: Some(side),
        price,
        qty,
        leaves: qty,
        ..Event::of(EventKind::Rested)
    }
}

fn canceled(id: u64, side: Side, price: i64, qty: i64, reason: Reason) -> Event {
    Event {
        reason,
        order: id,
        side: Some(side),
        price,
        qty,
        ..Event::of(EventKind::Canceled)
    }
}

#[allow(clippy::too_many_arguments)]
fn trade(
    taker: u64,
    side: Side,
    price: i64,
    qty: i64,
    leaves: i64,
    maker: u64,
    maker_leaves: i64,
) -> Event {
    Event {
        order: taker,
        side: Some(side),
        price,
        qty,
        leaves,
        maker,
        maker_leaves,
        ..Event::of(EventKind::Trade)
    }
}

fn rejected(o: &Order, reason: Reason) -> Event {
    Event {
        reason,
        order: o.id,
        side: Some(o.side),
        price: o.price,
        qty: o.qty,
        ..Event::of(EventKind::Rejected)
    }
}

fn rest(book: &mut Book, orders: &[Order]) {
    for o in orders {
        let ev = book.submit(*o);
        assert_eq!(
            ev.last().map(|e| e.kind),
            Some(EventKind::Rested),
            "{o:?} did not rest: {ev:?}"
        );
    }
}

#[test]
fn best_price_first_then_oldest_first_at_the_maker_price() {
    let mut b = Book::new();
    rest(
        &mut b,
        &[
            Order::limit(1, Side::Sell, 101, 5),
            Order::limit(2, Side::Sell, 100, 5),
            Order::limit(3, Side::Sell, 100, 5),
        ],
    );
    let o = Order::limit(4, Side::Buy, 101, 12);

    assert_eq!(
        b.submit(o),
        [
            accepted(&o),
            trade(4, Side::Buy, 100, 5, 7, 2, 0),
            trade(4, Side::Buy, 100, 5, 2, 3, 0),
            trade(4, Side::Buy, 101, 2, 0, 1, 3),
        ]
    );
    assert_eq!(
        b.best_level(Side::Sell),
        Some(PriceLevel {
            price: 101,
            qty: 3,
            orders: 1
        })
    );
}

#[test]
fn a_limit_order_that_does_not_cross_rests() {
    let mut b = Book::new();
    rest(&mut b, &[Order::limit(1, Side::Sell, 101, 5)]);
    let o = Order::limit(2, Side::Buy, 100, 3);

    assert_eq!(b.submit(o), [accepted(&o), rested(2, Side::Buy, 100, 3)]);
    assert_eq!(b.len(), 2);
}

#[test]
fn market_and_ioc_remainders_are_canceled_never_rested() {
    let mut b = Book::new();
    rest(&mut b, &[Order::limit(1, Side::Sell, 100, 5)]);
    let m = Order::market(2, Side::Buy, 8);
    assert_eq!(
        b.submit(m),
        [
            accepted(&m),
            trade(2, Side::Buy, 100, 5, 3, 1, 0),
            canceled(2, Side::Buy, 0, 3, Reason::NoLiquidity),
        ]
    );

    rest(&mut b, &[Order::limit(3, Side::Sell, 105, 5)]);
    let ioc = Order::limit(4, Side::Buy, 104, 2).with_tif(TimeInForce::Ioc);
    assert_eq!(
        b.submit(ioc),
        [
            accepted(&ioc),
            canceled(4, Side::Buy, 104, 2, Reason::NoLiquidity)
        ]
    );
    assert!(b.lookup(4).is_none());
}

#[test]
fn fill_or_kill_is_all_or_nothing() {
    let mut b = Book::new();
    rest(
        &mut b,
        &[
            Order::limit(1, Side::Sell, 100, 5),
            Order::limit(2, Side::Sell, 101, 5),
        ],
    );

    let too_big = Order::limit(3, Side::Buy, 101, 11).with_tif(TimeInForce::Fok);
    assert_eq!(
        b.submit(too_big),
        [rejected(&too_big, Reason::FokNotFilled)]
    );
    assert_eq!(b.len(), 2, "a rejected FOK order changes nothing");

    let fits = Order::limit(4, Side::Buy, 101, 10).with_tif(TimeInForce::Fok);
    let ev = b.submit(fits);
    assert_eq!(ev.iter().filter(|e| e.kind == EventKind::Trade).count(), 2);
    assert!(b.is_empty());
}

#[test]
fn fill_or_kill_counts_self_trade_prevention_exactly() {
    let mut b = Book::new();
    rest(
        &mut b,
        &[
            Order::limit(1, Side::Sell, 100, 5).owned_by(7),
            Order::limit(2, Side::Sell, 100, 5),
        ],
    );
    // With cancel_newest the own order stops the match before order 2: not fillable.
    let newest = Order {
        stp: Stp::CancelNewest,
        ..Order::limit(3, Side::Buy, 100, 5)
            .owned_by(7)
            .with_tif(TimeInForce::Fok)
    };
    assert_eq!(b.submit(newest), [rejected(&newest, Reason::FokNotFilled)]);
    // With cancel_oldest the own order is canceled and order 2 fills it.
    let oldest = Order {
        stp: Stp::CancelOldest,
        ..Order::limit(4, Side::Buy, 100, 5)
            .owned_by(7)
            .with_tif(TimeInForce::Fok)
    };
    assert_eq!(
        b.submit(oldest),
        [
            accepted(&oldest),
            canceled(1, Side::Sell, 100, 5, Reason::SelfTrade),
            Event {
                owner: 7,
                ..trade(4, Side::Buy, 100, 5, 0, 2, 0)
            },
        ]
    );
}

#[test]
fn post_only_never_takes_liquidity() {
    let mut b = Book::new();
    rest(&mut b, &[Order::limit(1, Side::Sell, 100, 5)]);
    let crossing = Order {
        post_only: true,
        ..Order::limit(2, Side::Buy, 100, 1)
    };
    assert_eq!(
        b.submit(crossing),
        [rejected(&crossing, Reason::PostOnlyWouldCross)]
    );

    let passive = Order {
        post_only: true,
        ..Order::limit(3, Side::Buy, 99, 1)
    };
    assert_eq!(
        b.submit(passive),
        [accepted(&passive), rested(3, Side::Buy, 99, 1)]
    );
}

#[test]
fn self_trade_prevention_in_its_three_modes() {
    fn book() -> Book {
        // Resting asks: 10 (owner 7), then 11 (owner 8), both at 100.
        let mut b = Book::new();
        rest(
            &mut b,
            &[
                Order::limit(10, Side::Sell, 100, 2).owned_by(7),
                Order::limit(11, Side::Sell, 100, 2).owned_by(8),
            ],
        );
        b
    }
    let order = |stp| Order {
        stp,
        ..Order::limit(20, Side::Buy, 100, 3).owned_by(7)
    };

    let o = order(Stp::CancelNewest);
    assert_eq!(
        book().submit(o),
        [
            accepted(&o),
            canceled(20, Side::Buy, 100, 3, Reason::SelfTrade)
        ]
    );

    let o = order(Stp::CancelOldest);
    assert_eq!(
        book().submit(o),
        [
            accepted(&o),
            canceled(10, Side::Sell, 100, 2, Reason::SelfTrade),
            Event {
                owner: 7,
                maker_owner: 8,
                ..trade(20, Side::Buy, 100, 2, 1, 11, 0)
            },
            rested(20, Side::Buy, 100, 1),
        ]
    );

    let o = order(Stp::CancelBoth);
    assert_eq!(
        book().submit(o),
        [
            accepted(&o),
            canceled(10, Side::Sell, 100, 2, Reason::SelfTrade),
            canceled(20, Side::Buy, 100, 3, Reason::SelfTrade),
        ]
    );

    // Anonymous orders (owner 0) never trigger it.
    let mut b = Book::new();
    rest(&mut b, &[Order::limit(1, Side::Sell, 100, 1)]);
    let anon = Order::limit(2, Side::Buy, 100, 1);
    assert_eq!(b.submit(anon)[1].kind, EventKind::Trade);
}

#[test]
fn reducing_quantity_keeps_the_queue_position_anything_else_loses_it() {
    let mut b = Book::new();
    rest(
        &mut b,
        &[
            Order::limit(1, Side::Sell, 100, 5),
            Order::limit(2, Side::Sell, 100, 5),
        ],
    );

    b.modify(1, 100, 3); // in place
    let first: Vec<u64> = b.orders(Side::Sell).map(|o| o.id).collect();
    assert_eq!(first, [1, 2]);

    b.modify(1, 100, 4); // an increase is a cancel-replace: back of the queue
    let after: Vec<u64> = b.orders(Side::Sell).map(|o| o.id).collect();
    assert_eq!(after, [2, 1]);
}

#[test]
fn a_modify_that_crosses_trades_at_once_with_its_own_account() {
    let mut b = Book::new();
    rest(
        &mut b,
        &[
            Order::limit(1, Side::Buy, 99, 5).owned_by(1002),
            Order::limit(4, Side::Sell, 105, 1).owned_by(3001),
        ],
    );

    let ev = b.modify(4, 99, 1);

    assert_eq!(ev[0].kind, EventKind::Modified);
    assert_eq!(
        ev[1],
        Event {
            owner: 3001,
            maker_owner: 1002,
            ..trade(4, Side::Sell, 99, 1, 0, 1, 4)
        }
    );
}

#[test]
fn invalid_requests_are_rejected_and_change_nothing() {
    let mut b = Book::new();
    rest(&mut b, &[Order::limit(1, Side::Sell, 100, 5)]);
    let cases = [
        (Order::limit(0, Side::Buy, 100, 1), Reason::InvalidId),
        (Order::limit(2, Side::Buy, 0, 1), Reason::InvalidPrice),
        (Order::limit(2, Side::Buy, 100, 0), Reason::InvalidQty),
        (
            Order::market(2, Side::Buy, 1).with_tif(TimeInForce::Gtc),
            Reason::InvalidTif,
        ),
        (
            Order {
                post_only: true,
                ..Order::limit(2, Side::Buy, 99, 1).with_tif(TimeInForce::Ioc)
            },
            Reason::InvalidTif,
        ),
        (Order::limit(1, Side::Buy, 99, 1), Reason::DuplicateId),
    ];
    for (o, reason) in cases {
        assert_eq!(b.submit(o), [rejected(&o, reason)], "{o:?}");
    }
    assert_eq!(
        b.cancel(42),
        [Event {
            reason: Reason::UnknownOrder,
            order: 42,
            ..Event::of(EventKind::Rejected)
        }]
    );
    assert_eq!(b.modify(1, 100, 0)[0].reason, Reason::InvalidQty);
    assert_eq!(b.modify(1, 0, 1)[0].reason, Reason::InvalidPrice);
    assert_eq!(b.len(), 1);
    b.check_invariants();
}

#[test]
fn changes_give_the_new_total_of_every_touched_level() {
    let mut b = Book::new();
    rest(
        &mut b,
        &[
            Order::limit(1, Side::Sell, 100, 5),
            Order::limit(2, Side::Sell, 101, 5),
            Order::limit(3, Side::Buy, 98, 2),
        ],
    );

    b.submit(Order::limit(4, Side::Buy, 101, 7));

    assert_eq!(
        b.changes(),
        [
            LevelChange {
                side: Side::Sell,
                price: 100,
                qty: 0
            }, // gone
            LevelChange {
                side: Side::Sell,
                price: 101,
                qty: 3
            },
        ]
    );
}

#[test]
fn depth_lookup_and_clear() {
    let mut b = Book::new();
    rest(
        &mut b,
        &[
            Order::limit(1, Side::Buy, 98, 1),
            Order::limit(2, Side::Buy, 99, 2),
            Order::limit(3, Side::Buy, 99, 3),
        ],
    );

    let depth: Vec<PriceLevel> = b.depth(Side::Buy, 0).collect();
    assert_eq!(
        depth,
        [
            PriceLevel {
                price: 99,
                qty: 5,
                orders: 2
            },
            PriceLevel {
                price: 98,
                qty: 1,
                orders: 1
            }
        ]
    );
    assert_eq!(b.depth(Side::Buy, 1).count(), 1);
    assert_eq!(b.lookup(3).map(|o| o.qty), Some(3));
    assert_eq!(b.levels(Side::Buy), 2);

    b.clear();
    assert!(b.is_empty());
    assert_eq!(b.best_level(Side::Buy), None);
    rest(&mut b, &[Order::limit(1, Side::Buy, 98, 1)]); // ids can be reused after clear
}

#[test]
fn the_digest_depends_on_queue_positions() {
    let mut a = Book::new();
    let mut b = Book::new();
    rest(
        &mut a,
        &[
            Order::limit(1, Side::Buy, 99, 1),
            Order::limit(2, Side::Buy, 99, 1),
        ],
    );
    rest(
        &mut b,
        &[
            Order::limit(2, Side::Buy, 99, 1),
            Order::limit(1, Side::Buy, 99, 1),
        ],
    );
    assert_ne!(a.digest(), b.digest());

    let mut c = Book::new();
    rest(
        &mut c,
        &[
            Order::limit(1, Side::Buy, 99, 1),
            Order::limit(2, Side::Buy, 99, 1),
        ],
    );
    assert_eq!(a.digest(), c.digest());
}

//! A deliberately naive order book, used as the oracle of the property tests and the
//! fuzzer: one unsorted vector, a linear scan for the best order on every step, and
//! fill-or-kill decided by trying the match on a copy. It is slow and obviously correct;
//! [`crate::Book`] must produce exactly the same events. Only [`crate::validate`] is
//! shared. It is a port of the reference model of the Go implementation.

use crate::types::{
    Event, EventKind, Order, OrderId, OrderInfo, OrderType, Reason, Side, Stp, TimeInForce,
    acceptable, validate,
};

#[derive(Clone, Copy, Debug)]
struct RefOrder {
    info: OrderInfo,
    seq: u64, // arrival order, for time priority
}

/// The naive book.
#[derive(Clone, Debug, Default)]
pub struct RefBook {
    orders: Vec<RefOrder>,
    seq: u64,
    events: Vec<Event>,
}

impl RefBook {
    /// An empty book.
    pub fn new() -> RefBook {
        RefBook::default()
    }

    fn find(&self, id: OrderId) -> Option<usize> {
        self.orders.iter().position(|o| o.info.id == id)
    }

    /// The index of the order with the highest priority on side `s`.
    fn best(&self, s: Side) -> Option<usize> {
        let mut best: Option<usize> = None;
        for (i, o) in self.orders.iter().enumerate() {
            if o.info.side != s {
                continue;
            }
            let better = match best {
                None => true,
                Some(b) => {
                    let b = &self.orders[b];
                    let price_better = match s {
                        Side::Buy => o.info.price > b.info.price,
                        Side::Sell => o.info.price < b.info.price,
                    };
                    price_better || (o.info.price == b.info.price && o.seq < b.seq)
                }
            };
            if better {
                best = Some(i);
            }
        }
        best
    }

    fn reject(
        &mut self,
        id: OrderId,
        side: Option<Side>,
        price: i64,
        qty: i64,
        reason: Reason,
    ) -> &[Event] {
        self.events.push(Event {
            reason,
            order: id,
            side,
            price,
            qty,
            ..Event::of(EventKind::Rejected)
        });
        &self.events
    }

    /// Same contract as [`crate::Book::submit`].
    pub fn submit(&mut self, mut o: Order) -> &[Event] {
        self.events.clear();
        let reason = validate(&o);
        if reason != Reason::None {
            return self.reject(o.id, Some(o.side), o.price, o.qty, reason);
        }
        if self.find(o.id).is_some() {
            return self.reject(o.id, Some(o.side), o.price, o.qty, Reason::DuplicateId);
        }
        if o.kind == OrderType::Market {
            o.price = 0;
        }
        if o.post_only {
            if let Some(i) = self.best(o.side.opposite()) {
                if acceptable(o.side, o.price, self.orders[i].info.price) {
                    return self.reject(
                        o.id,
                        Some(o.side),
                        o.price,
                        o.qty,
                        Reason::PostOnlyWouldCross,
                    );
                }
            }
        }
        if o.tif == TimeInForce::Fok {
            let mut trial = self.clone();
            let mut copy = o;
            trial.match_order(&mut copy);
            if copy.qty > 0 {
                return self.reject(o.id, Some(o.side), o.price, o.qty, Reason::FokNotFilled);
            }
        }
        self.events.push(Event {
            order: o.id,
            side: Some(o.side),
            price: o.price,
            qty: o.qty,
            leaves: o.qty,
            ..Event::of(EventKind::Accepted)
        });
        let stopped = self.match_order(&mut o);
        self.finish(&o, stopped);
        &self.events
    }

    fn match_order(&mut self, o: &mut Order) -> bool {
        while o.qty > 0 {
            let Some(i) = self.best(o.side.opposite()) else {
                return false;
            };
            let m = self.orders[i].info;
            if o.kind == OrderType::Limit && !acceptable(o.side, o.price, m.price) {
                return false;
            }
            if o.owner != 0 && m.owner == o.owner {
                if o.stp == Stp::CancelNewest {
                    return true;
                }
                self.cancel_at(i, Reason::SelfTrade);
                if o.stp == Stp::CancelBoth {
                    return true;
                }
                continue;
            }
            let q = o.qty.min(m.qty);
            o.qty -= q;
            self.orders[i].info.qty -= q;
            self.events.push(Event {
                order: o.id,
                side: Some(o.side),
                price: m.price,
                qty: q,
                leaves: o.qty,
                maker: m.id,
                maker_leaves: m.qty - q,
                owner: o.owner,
                maker_owner: m.owner,
                ..Event::of(EventKind::Trade)
            });
            if m.qty - q == 0 {
                self.orders.remove(i);
            }
        }
        false
    }

    fn finish(&mut self, o: &Order, stopped: bool) {
        if o.qty == 0 {
            return;
        }
        let base = Event {
            order: o.id,
            side: Some(o.side),
            price: o.price,
            qty: o.qty,
            ..Event::of(EventKind::Canceled)
        };
        if stopped {
            self.events.push(Event {
                reason: Reason::SelfTrade,
                ..base
            });
        } else if o.kind == OrderType::Limit && o.tif == TimeInForce::Gtc {
            self.seq += 1;
            self.orders.push(RefOrder {
                info: OrderInfo {
                    id: o.id,
                    owner: o.owner,
                    side: o.side,
                    price: o.price,
                    qty: o.qty,
                    post_only: o.post_only,
                    stp: o.stp,
                },
                seq: self.seq,
            });
            self.events.push(Event {
                kind: EventKind::Rested,
                leaves: o.qty,
                ..base
            });
        } else {
            self.events.push(Event {
                reason: Reason::NoLiquidity,
                ..base
            });
        }
    }

    fn cancel_at(&mut self, i: usize, reason: Reason) {
        let m = self.orders.remove(i).info;
        self.events.push(Event {
            reason,
            order: m.id,
            side: Some(m.side),
            price: m.price,
            qty: m.qty,
            ..Event::of(EventKind::Canceled)
        });
    }

    /// Same contract as [`crate::Book::cancel`].
    pub fn cancel(&mut self, id: OrderId) -> &[Event] {
        self.events.clear();
        match self.find(id) {
            None => self.reject(id, None, 0, 0, Reason::UnknownOrder),
            Some(i) => {
                self.cancel_at(i, Reason::CanceledByUser);
                &self.events
            }
        }
    }

    /// Same contract as [`crate::Book::modify`].
    pub fn modify(&mut self, id: OrderId, price: i64, qty: i64) -> &[Event] {
        self.events.clear();
        let Some(i) = self.find(id) else {
            return self.reject(id, None, price, qty, Reason::UnknownOrder);
        };
        let m = self.orders[i].info;
        if qty <= 0 {
            return self.reject(id, Some(m.side), price, qty, Reason::InvalidQty);
        }
        if price <= 0 {
            return self.reject(id, Some(m.side), price, qty, Reason::InvalidPrice);
        }
        let modified = Event {
            order: id,
            side: Some(m.side),
            price,
            qty,
            leaves: qty,
            ..Event::of(EventKind::Modified)
        };
        if price == m.price && qty <= m.qty {
            self.orders[i].info.qty = qty; // keeps its seq, so it keeps its place
            self.events.push(modified);
            return &self.events;
        }
        if m.post_only {
            if let Some(j) = self.best(m.side.opposite()) {
                if acceptable(m.side, price, self.orders[j].info.price) {
                    return self.reject(id, Some(m.side), price, qty, Reason::PostOnlyWouldCross);
                }
            }
        }
        self.orders.remove(i);
        self.events.push(modified);
        let mut o = Order {
            id,
            owner: m.owner,
            side: m.side,
            kind: OrderType::Limit,
            tif: TimeInForce::Gtc,
            price,
            qty,
            post_only: m.post_only,
            stp: m.stp,
        };
        let stopped = self.match_order(&mut o);
        self.finish(&o, stopped);
        &self.events
    }

    /// The orders of one side in priority order, like [`crate::Book::orders`].
    pub fn sorted(&self, s: Side) -> Vec<OrderInfo> {
        let mut os: Vec<RefOrder> = self
            .orders
            .iter()
            .filter(|o| o.info.side == s)
            .copied()
            .collect();
        os.sort_by(|a, b| {
            let by_price = match s {
                Side::Buy => b.info.price.cmp(&a.info.price),
                Side::Sell => a.info.price.cmp(&b.info.price),
            };
            by_price.then(a.seq.cmp(&b.seq))
        });
        os.into_iter().map(|o| o.info).collect()
    }
}

/// One book command, for tests that drive several books with the same input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// [`crate::Book::submit`].
    Submit(Order),
    /// [`crate::Book::cancel`].
    Cancel(OrderId),
    /// [`crate::Book::modify`].
    Modify(OrderId, i64, i64),
}

impl Command {
    /// Runs the command on a book.
    pub fn apply<'a>(&self, book: &'a mut crate::Book) -> &'a [Event] {
        match *self {
            Command::Submit(o) => book.submit(o),
            Command::Cancel(id) => book.cancel(id),
            Command::Modify(id, price, qty) => book.modify(id, price, qty),
        }
    }

    /// Runs the command on the reference book.
    pub fn apply_reference<'a>(&self, book: &'a mut RefBook) -> &'a [Event] {
        match *self {
            Command::Submit(o) => book.submit(o),
            Command::Cancel(id) => book.cancel(id),
            Command::Modify(id, price, qty) => book.modify(id, price, qty),
        }
    }
}

/// Turns arbitrary bytes into commands, 8 bytes each, over a narrow price band and few
/// ids and owners, so collisions (crossing prices, duplicate ids, self-trades, cancels of
/// live orders) are frequent. The fuzzer uses it.
pub fn decode(data: &[u8]) -> Vec<Command> {
    data.chunks_exact(8)
        .map(|c| {
            let id = OrderId::from(c[1] % 40); // includes 0: an invalid id
            let price = 95 + i64::from(c[2] % 11);
            let qty = i64::from(c[3] % 12); // includes 0: an invalid quantity
            match c[0] % 10 {
                0 => Command::Cancel(id),
                1 => Command::Modify(id, price, qty),
                op => {
                    let side = if c[4] & 1 == 0 { Side::Buy } else { Side::Sell };
                    let kind = if op == 2 {
                        OrderType::Market
                    } else {
                        OrderType::Limit
                    };
                    let tif = match c[5] % 3 {
                        0 => TimeInForce::Gtc,
                        1 => TimeInForce::Ioc,
                        _ => TimeInForce::Fok,
                    };
                    let stp = match c[6] % 3 {
                        0 => Stp::CancelNewest,
                        1 => Stp::CancelOldest,
                        _ => Stp::CancelBoth,
                    };
                    Command::Submit(Order {
                        id,
                        owner: u64::from(c[7] % 4), // 0 = anonymous
                        side,
                        kind,
                        tif,
                        price,
                        qty,
                        post_only: c[4] & 2 != 0,
                        stp,
                    })
                }
            }
        })
        .collect()
}

/// Runs the commands on [`crate::Book`] and on [`RefBook`] and panics at the first
/// difference in events or resting orders, or at a broken invariant.
///
/// # Panics
///
/// When the two books disagree.
pub fn check_against_reference(commands: &[Command]) {
    let mut book = crate::Book::new();
    let mut reference = RefBook::new();
    for (step, cmd) in commands.iter().enumerate() {
        let got = cmd.apply(&mut book).to_vec();
        let want = cmd.apply_reference(&mut reference);
        assert_eq!(got, want, "step {step}: {cmd:?}");
        book.changes();
        book.check_invariants();
        for s in [Side::Buy, Side::Sell] {
            let resting: Vec<OrderInfo> = book.orders(s).collect();
            assert_eq!(
                resting,
                reference.sorted(s),
                "step {step}: {s:?} side differs after {cmd:?}"
            );
        }
    }
}

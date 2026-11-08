//! The order book.

use std::cmp::min;
use std::collections::HashMap;

use crate::hash::{BuildIdHasher, Fnv1a};
use crate::types::{
    Event, EventKind, LevelChange, Order, OrderId, OrderInfo, OrderType, PriceLevel, Reason, Side,
    Stp, TimeInForce, acceptable, validate,
};

/// "No node": the end of a list, or a missing link. Indices are `u32` to keep nodes small.
const NIL: u32 = u32::MAX;

/// A resting order: a node of the doubly linked FIFO list of its price level, so that a
/// cancel unlinks it in O(1). Nodes live in an arena (`Book::nodes`) and point to each
/// other by index, not by reference: see docs/adr/0001-arena-and-indices.md.
#[derive(Clone, Copy, Debug)]
struct Node {
    id: OrderId,
    owner: u64,
    price: i64,
    qty: i64,
    side: Side,
    post_only: bool,
    stp: Stp,
    level: u32,
    prev: u32,
    next: u32,
}

impl Node {
    const fn info(&self) -> OrderInfo {
        OrderInfo {
            id: self.id,
            owner: self.owner,
            side: self.side,
            price: self.price,
            qty: self.qty,
            post_only: self.post_only,
            stp: self.stp,
        }
    }
}

/// The FIFO queue of orders at one price.
#[derive(Clone, Copy, Debug)]
struct Level {
    price: i64,
    qty: i64, // sum of the open quantity of its orders
    n: u32,   // number of orders
    head: u32,
    tail: u32,
    epoch: u64, // last operation that recorded a change to this level
}

impl Level {
    const EMPTY: Level = Level {
        price: 0,
        qty: 0,
        n: 0,
        head: NIL,
        tail: NIL,
        epoch: 0,
    };
}

/// A limit order book for one instrument, with price-time priority.
///
/// Prices are integer ticks and quantities integer lots. Every operation returns the
/// events it caused, in a buffer that is reused: the slice is valid until the next call.
/// In steady state (once its buffers have grown to the working size) the book does not
/// allocate.
#[derive(Clone, Debug)]
pub struct Book {
    /// Level indices sorted from the worst price to the best, so the best is the last
    /// element: O(1) to read, O(1) to remove when it empties, and new levels near the top
    /// of the book shift only a few entries. Same layout as the Go implementation.
    bids: Vec<u32>,
    asks: Vec<u32>,
    nodes: Vec<Node>,
    free_nodes: Vec<u32>,
    levels: Vec<Level>,
    free_levels: Vec<u32>,
    index: HashMap<OrderId, u32, BuildIdHasher>,
    events: Vec<Event>,
    touched: Vec<LevelChange>, // levels changed by the current operation; qty filled in by changes()
    changes: Vec<LevelChange>,
    epoch: u64,
}

impl Default for Book {
    fn default() -> Self {
        Self::new()
    }
}

impl Book {
    /// An empty book.
    pub fn new() -> Book {
        Book {
            bids: Vec::new(),
            asks: Vec::new(),
            nodes: Vec::new(),
            free_nodes: Vec::new(),
            levels: Vec::new(),
            free_levels: Vec::new(),
            index: HashMap::default(),
            events: Vec::new(),
            touched: Vec::new(),
            changes: Vec::new(),
            epoch: 0,
        }
    }

    /// Empties the book but keeps its allocated memory, to start a new session without
    /// allocating again.
    pub fn clear(&mut self) {
        self.bids.clear();
        self.asks.clear();
        self.nodes.clear();
        self.free_nodes.clear();
        self.levels.clear();
        self.free_levels.clear();
        self.index.clear();
        self.events.clear();
        self.touched.clear();
        self.changes.clear();
    }

    // ------------------------------------------------------------------ commands

    /// Matches a new order against the book. What is left of a GTC limit order rests on
    /// the book; what is left of an IOC or market order is canceled.
    pub fn submit(&mut self, mut o: Order) -> &[Event] {
        self.begin();
        let r = validate(&o);
        if r != Reason::None {
            return self.reject(o.id, Some(o.side), o.price, o.qty, r);
        }
        if self.index.contains_key(&o.id) {
            return self.reject(o.id, Some(o.side), o.price, o.qty, Reason::DuplicateId);
        }
        if o.kind == OrderType::Market {
            o.price = 0;
        }
        if o.post_only && self.crosses(o.side, o.price) {
            return self.reject(
                o.id,
                Some(o.side),
                o.price,
                o.qty,
                Reason::PostOnlyWouldCross,
            );
        }
        if o.tif == TimeInForce::Fok && !self.fillable(&o) {
            return self.reject(o.id, Some(o.side), o.price, o.qty, Reason::FokNotFilled);
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

    /// Removes a resting order.
    pub fn cancel(&mut self, id: OrderId) -> &[Event] {
        self.begin();
        match self.index.get(&id) {
            None => self.reject(id, None, 0, 0, Reason::UnknownOrder),
            Some(&m) => {
                self.cancel_resting(m, Reason::CanceledByUser);
                &self.events
            }
        }
    }

    /// Changes the price and the open quantity of a resting order.
    ///
    /// Lowering the quantity at the same price keeps the order's place in the queue. Any
    /// other change is a cancel-replace: the order goes to the back of the queue at the
    /// new price and trades at once if the new price crosses the spread (a post-only
    /// order is rejected instead, and stays as it was).
    pub fn modify(&mut self, id: OrderId, price: i64, qty: i64) -> &[Event] {
        self.begin();
        let Some(&m) = self.index.get(&id) else {
            return self.reject(id, None, price, qty, Reason::UnknownOrder);
        };
        let node = self.nodes[m as usize];
        if qty <= 0 {
            return self.reject(id, Some(node.side), price, qty, Reason::InvalidQty);
        }
        if price <= 0 {
            return self.reject(id, Some(node.side), price, qty, Reason::InvalidPrice);
        }
        let modified = Event {
            order: id,
            side: Some(node.side),
            price,
            qty,
            leaves: qty,
            ..Event::of(EventKind::Modified)
        };
        if price == node.price && qty <= node.qty {
            let d = node.qty - qty;
            if d > 0 {
                self.nodes[m as usize].qty = qty;
                self.levels[node.level as usize].qty -= d;
                self.touch(node.side, node.level);
            }
            self.events.push(modified);
            return &self.events;
        }
        if node.post_only && self.crosses(node.side, price) {
            return self.reject(id, Some(node.side), price, qty, Reason::PostOnlyWouldCross);
        }
        let mut o = Order {
            id,
            owner: node.owner,
            side: node.side,
            kind: OrderType::Limit,
            tif: TimeInForce::Gtc,
            price,
            qty,
            post_only: node.post_only,
            stp: node.stp,
        };
        self.remove(m);
        self.events.push(modified);
        let stopped = self.match_order(&mut o);
        self.finish(&o, stopped);
        &self.events
    }

    // ------------------------------------------------------------------ matching

    fn begin(&mut self) {
        self.events.clear();
        self.touched.clear();
        self.epoch += 1;
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

    fn side_levels(&self, s: Side) -> &Vec<u32> {
        match s {
            Side::Buy => &self.bids,
            Side::Sell => &self.asks,
        }
    }

    fn side_levels_mut(&mut self, s: Side) -> &mut Vec<u32> {
        match s {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        }
    }

    fn best(&self, s: Side) -> Option<u32> {
        self.side_levels(s).last().copied()
    }

    /// Maps a price so that a higher rank is a better price for the side.
    const fn rank(s: Side, p: i64) -> i64 {
        match s {
            Side::Buy => p,
            Side::Sell => -p,
        }
    }

    /// The position of the level at price `p`, or where it would go.
    fn search(&self, s: Side, p: i64) -> (usize, bool) {
        let levels = self.side_levels(s);
        let r = Self::rank(s, p);
        if let Some(&top) = levels.last() {
            // fast path: most activity is at the top of the book
            let top_rank = Self::rank(s, self.levels[top as usize].price);
            if r == top_rank {
                return (levels.len() - 1, true);
            }
            if r > top_rank {
                return (levels.len(), false);
            }
        }
        let i = levels.partition_point(|&l| Self::rank(s, self.levels[l as usize].price) < r);
        (
            i,
            i < levels.len() && self.levels[levels[i] as usize].price == p,
        )
    }

    fn touch(&mut self, s: Side, level: u32) {
        let lvl = &mut self.levels[level as usize];
        if lvl.epoch != self.epoch {
            lvl.epoch = self.epoch;
            self.touched.push(LevelChange {
                side: s,
                price: lvl.price,
                qty: 0,
            });
        }
    }

    /// Whether a limit order at `price` would trade at once.
    fn crosses(&self, s: Side, price: i64) -> bool {
        self.best(s.opposite())
            .is_some_and(|l| acceptable(s, price, self.levels[l as usize].price))
    }

    /// Whether matching `o` now would fill it completely. It walks the queue in the same
    /// order as `match_order`, so a resting order of the same owner stops the walk exactly
    /// where self-trade prevention would stop the match.
    fn fillable(&self, o: &Order) -> bool {
        let mut need = o.qty;
        for &l in self.side_levels(o.side.opposite()).iter().rev() {
            let lvl = &self.levels[l as usize];
            if o.kind == OrderType::Limit && !acceptable(o.side, o.price, lvl.price) {
                return false;
            }
            if o.owner == 0 {
                // no self-trade prevention: the level total is enough
                if lvl.qty >= need {
                    return true;
                }
                need -= lvl.qty;
                continue;
            }
            let mut m = lvl.head;
            while m != NIL {
                let node = &self.nodes[m as usize];
                if node.owner == o.owner {
                    if o.stp != Stp::CancelOldest {
                        return false;
                    }
                    // it would be canceled, not traded with
                } else {
                    if node.qty >= need {
                        return true;
                    }
                    need -= node.qty;
                }
                m = node.next;
            }
        }
        false
    }

    /// Trades `o` against the opposite side, best price first and oldest order first
    /// within a price, until `o` is filled, no acceptable price is left, or self-trade
    /// prevention stops it. Returns whether self-trade prevention stopped it.
    fn match_order(&mut self, o: &mut Order) -> bool {
        let opp = o.side.opposite();
        while o.qty > 0 {
            let Some(l) = self.best(opp) else {
                return false;
            };
            let price = self.levels[l as usize].price;
            if o.kind == OrderType::Limit && !acceptable(o.side, o.price, price) {
                return false;
            }
            let mut m = self.levels[l as usize].head;
            while m != NIL && o.qty > 0 {
                let node = self.nodes[m as usize];
                let next = node.next;
                if o.owner != 0 && node.owner == o.owner {
                    match o.stp {
                        Stp::CancelNewest => return true,
                        Stp::CancelOldest => {
                            self.cancel_resting(m, Reason::SelfTrade);
                            m = next;
                            continue;
                        }
                        Stp::CancelBoth => {
                            self.cancel_resting(m, Reason::SelfTrade);
                            return true;
                        }
                    }
                }
                let q = min(o.qty, node.qty);
                o.qty -= q;
                let maker_leaves = node.qty - q;
                self.nodes[m as usize].qty = maker_leaves;
                self.levels[l as usize].qty -= q;
                self.touch(opp, l);
                self.events.push(Event {
                    order: o.id,
                    side: Some(o.side),
                    price,
                    qty: q,
                    leaves: o.qty,
                    maker: node.id,
                    maker_leaves,
                    owner: o.owner,
                    maker_owner: node.owner,
                    ..Event::of(EventKind::Trade)
                });
                if maker_leaves == 0 {
                    self.remove(m); // frees the level too when it was its last order
                }
                m = next;
            }
        }
        false
    }

    /// Decides what happens to the part of `o` that did not trade.
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
            self.rest(o);
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

    fn rest(&mut self, o: &Order) {
        let (i, found) = self.search(o.side, o.price);
        let l = if found {
            self.side_levels(o.side)[i]
        } else {
            let l = self.new_level(Level {
                price: o.price,
                ..Level::EMPTY
            });
            self.side_levels_mut(o.side).insert(i, l);
            l
        };
        let tail = self.levels[l as usize].tail;
        let m = self.new_node(Node {
            id: o.id,
            owner: o.owner,
            price: o.price,
            qty: o.qty,
            side: o.side,
            post_only: o.post_only,
            stp: o.stp,
            level: l,
            prev: tail,
            next: NIL,
        });
        if tail == NIL {
            self.levels[l as usize].head = m;
        } else {
            self.nodes[tail as usize].next = m;
        }
        let lvl = &mut self.levels[l as usize];
        lvl.tail = m;
        lvl.qty += o.qty;
        lvl.n += 1;
        self.touch(o.side, l);
        self.index.insert(o.id, m);
    }

    fn cancel_resting(&mut self, m: u32, reason: Reason) {
        let n = self.nodes[m as usize];
        self.events.push(Event {
            reason,
            order: n.id,
            side: Some(n.side),
            price: n.price,
            qty: n.qty,
            ..Event::of(EventKind::Canceled)
        });
        self.remove(m);
    }

    /// Unlinks a resting order, and its level if the level becomes empty.
    fn remove(&mut self, m: u32) {
        let n = self.nodes[m as usize];
        let l = n.level;
        if n.prev == NIL {
            self.levels[l as usize].head = n.next;
        } else {
            self.nodes[n.prev as usize].next = n.next;
        }
        if n.next == NIL {
            self.levels[l as usize].tail = n.prev;
        } else {
            self.nodes[n.next as usize].prev = n.prev;
        }
        let lvl = &mut self.levels[l as usize];
        lvl.qty -= n.qty;
        lvl.n -= 1;
        let (empty, price) = (lvl.n == 0, lvl.price);
        self.touch(n.side, l);
        self.index.remove(&n.id);
        if empty {
            let (i, _) = self.search(n.side, price);
            self.side_levels_mut(n.side).remove(i);
            self.levels[l as usize] = Level::EMPTY;
            self.free_levels.push(l);
        }
        self.free_nodes.push(m);
    }

    fn new_node(&mut self, node: Node) -> u32 {
        if let Some(m) = self.free_nodes.pop() {
            self.nodes[m as usize] = node;
            m
        } else {
            self.nodes.push(node);
            (self.nodes.len() - 1) as u32
        }
    }

    fn new_level(&mut self, level: Level) -> u32 {
        if let Some(l) = self.free_levels.pop() {
            self.levels[l as usize] = level;
            l
        } else {
            self.levels.push(level);
            (self.levels.len() - 1) as u32
        }
    }

    // ------------------------------------------------------------------ queries

    /// The levels changed by the last operation, sorted by side and price, with their new
    /// total quantity (0 = gone). Applying them in order to the previous depth gives the
    /// new depth, which is what incremental market data needs.
    pub fn changes(&mut self) -> &[LevelChange] {
        self.touched.sort_unstable_by_key(|c| (c.side, c.price));
        self.touched.dedup_by_key(|c| (c.side, c.price));
        self.changes.clear();
        for i in 0..self.touched.len() {
            let mut t = self.touched[i];
            if let (j, true) = self.search(t.side, t.price) {
                t.qty = self.levels[self.side_levels(t.side)[j] as usize].qty;
            }
            self.changes.push(t);
        }
        &self.changes
    }

    /// Number of resting orders.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Whether no order rests on the book.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Number of price levels on one side.
    pub fn levels(&self, s: Side) -> usize {
        self.side_levels(s).len()
    }

    /// The best level of one side.
    pub fn best_level(&self, s: Side) -> Option<PriceLevel> {
        self.best(s).map(|l| self.price_level(l))
    }

    /// Up to `n` levels of one side, best first; `n == 0` means all.
    pub fn depth(&self, s: Side, n: usize) -> impl Iterator<Item = PriceLevel> + '_ {
        let levels = self.side_levels(s);
        let n = if n == 0 { levels.len() } else { n };
        levels.iter().rev().take(n).map(|&l| self.price_level(l))
    }

    fn price_level(&self, l: u32) -> PriceLevel {
        let lvl = &self.levels[l as usize];
        PriceLevel {
            price: lvl.price,
            qty: lvl.qty,
            orders: lvl.n,
        }
    }

    /// A resting order.
    pub fn lookup(&self, id: OrderId) -> Option<OrderInfo> {
        self.index.get(&id).map(|&m| self.nodes[m as usize].info())
    }

    /// The resting orders of one side in priority order: best price first, oldest first
    /// within a price.
    pub fn orders(&self, s: Side) -> impl Iterator<Item = OrderInfo> + '_ {
        self.side_levels(s).iter().rev().flat_map(move |&l| {
            let mut m = self.levels[l as usize].head;
            std::iter::from_fn(move || {
                (m != NIL).then(|| {
                    let node = &self.nodes[m as usize];
                    m = node.next;
                    node.info()
                })
            })
        })
    }

    /// A hash of the whole book state, queue positions included. Two books that got the
    /// same commands have the same digest, and it is the same digest the Go
    /// implementation computes (FNV-1a over the same fields in the same order).
    pub fn digest(&self) -> u64 {
        let mut h = Fnv1a::default();
        for s in [Side::Buy, Side::Sell] {
            h.put(s as u64);
            for o in self.orders(s) {
                h.put(o.id);
                h.put(o.owner);
                h.put(o.price as u64);
                h.put(o.qty as u64);
                h.put(((o.stp as u64) << 1) | u64::from(o.post_only));
            }
        }
        h.sum()
    }

    /// Checks the internal invariants (levels sorted, totals equal to the sum of their
    /// orders, links consistent, index complete). For tests; O(n).
    ///
    /// # Panics
    ///
    /// When an invariant is broken.
    pub fn check_invariants(&self) {
        let mut seen = 0;
        for s in [Side::Buy, Side::Sell] {
            let levels = self.side_levels(s);
            for w in levels.windows(2) {
                let (a, b) = (
                    self.levels[w[0] as usize].price,
                    self.levels[w[1] as usize].price,
                );
                assert!(
                    Self::rank(s, a) < Self::rank(s, b),
                    "{s:?} levels out of order: {a} before {b}"
                );
            }
            for &l in levels {
                let lvl = &self.levels[l as usize];
                let (mut total, mut count, mut prev, mut m) = (0, 0, NIL, lvl.head);
                while m != NIL {
                    let node = &self.nodes[m as usize];
                    assert_eq!(node.prev, prev, "broken back link at {}", node.id);
                    assert_eq!(node.level, l, "order {} points to the wrong level", node.id);
                    assert_eq!(
                        (node.price, node.side),
                        (lvl.price, s),
                        "order {} on the wrong level",
                        node.id
                    );
                    assert!(node.qty > 0, "order {} rests with no quantity", node.id);
                    assert_eq!(
                        self.index.get(&node.id),
                        Some(&m),
                        "order {} not indexed",
                        node.id
                    );
                    total += node.qty;
                    count += 1;
                    prev = m;
                    m = node.next;
                }
                assert_eq!(lvl.tail, prev, "tail of level {}", lvl.price);
                assert_eq!(
                    (lvl.qty, lvl.n),
                    (total, count),
                    "totals of level {}",
                    lvl.price
                );
                assert!(count > 0, "empty level {} left on the book", lvl.price);
                seen += count as usize;
            }
        }
        assert_eq!(seen, self.index.len(), "index and levels disagree");
    }
}

#[cfg(test)]
mod tests {
    use super::{Level, Node};

    #[test]
    fn nodes_stay_small() {
        // 4 × 8 bytes of order data, 3 one-byte enums and flags, 3 four-byte links: 48 bytes.
        assert_eq!(size_of::<Node>(), 48);
        assert_eq!(size_of::<Level>(), 40);
    }
}

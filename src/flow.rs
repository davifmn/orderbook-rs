//! Reader for recorded command flows (`OBFLOW01` files) written by matching-engine's
//! `cmd/flowdump`: the commands, plus what the Go book did with them.

use std::fmt;

use crate::reference::Command;
use crate::types::{Order, OrderType, Side, Stp, TimeInForce};

/// What the Go implementation produced for the flow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expected {
    /// Number of events.
    pub events: u64,
    /// Number of trades.
    pub trades: u64,
    /// `Book::digest` after the last command.
    pub digest: u64,
    /// FNV-1a over every event's fields (see `Event::fields`).
    pub event_hash: u64,
    /// The event hash after every `checkpoint_every` commands.
    pub checkpoints: Vec<u64>,
    /// Commands between checkpoints.
    pub checkpoint_every: usize,
}

/// A recorded flow.
#[derive(Clone, Debug)]
pub struct Flow {
    /// The commands, in order.
    pub commands: Vec<Command>,
    /// The Go book's results.
    pub expected: Expected,
}

/// Why a flow file could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatError(String);

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "flow file: {}", self.0)
    }
}

impl std::error::Error for FormatError {}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], FormatError> {
        let end = self.pos + N;
        let bytes = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| FormatError(format!("truncated at byte {}", self.pos)))?;
        self.pos = end;
        Ok(bytes.try_into().expect("slice of length N"))
    }
    fn u8(&mut self) -> Result<u8, FormatError> {
        Ok(self.take::<1>()?[0])
    }
    fn u32(&mut self) -> Result<u32, FormatError> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn u64(&mut self) -> Result<u64, FormatError> {
        Ok(u64::from_le_bytes(self.take()?))
    }
    fn i64(&mut self) -> Result<i64, FormatError> {
        Ok(i64::from_le_bytes(self.take()?))
    }
}

fn enum_value<T>(v: u8, what: &str, values: &[(u8, T)]) -> Result<T, FormatError>
where
    T: Copy,
{
    values
        .iter()
        .find(|(n, _)| *n == v)
        .map(|&(_, t)| t)
        .ok_or_else(|| FormatError(format!("invalid {what} {v}")))
}

impl Flow {
    /// Parses a flow file.
    ///
    /// # Errors
    ///
    /// A wrong magic number, an unknown command or enum value, or a truncated file.
    pub fn parse(data: &[u8]) -> Result<Flow, FormatError> {
        let mut r = Reader { data, pos: 0 };
        if r.take::<8>()? != *b"OBFLOW01" {
            return Err(FormatError("not an OBFLOW01 file".into()));
        }
        let count = r.u32()? as usize;
        let checkpoint_every = r.u32()? as usize;
        let mut commands = Vec::with_capacity(count);
        for _ in 0..count {
            let cmd = match r.u8()? {
                1 => {
                    let id = r.u64()?;
                    let owner = r.u64()?;
                    let side = enum_value(r.u8()?, "side", &[(1, Side::Buy), (2, Side::Sell)])?;
                    let kind = enum_value(
                        r.u8()?,
                        "type",
                        &[(1, OrderType::Limit), (2, OrderType::Market)],
                    )?;
                    let tif = enum_value(
                        r.u8()?,
                        "time in force",
                        &[
                            (1, TimeInForce::Gtc),
                            (2, TimeInForce::Ioc),
                            (3, TimeInForce::Fok),
                        ],
                    )?;
                    let stp = enum_value(
                        r.u8()?,
                        "stp",
                        &[
                            (0, Stp::CancelNewest),
                            (1, Stp::CancelOldest),
                            (2, Stp::CancelBoth),
                        ],
                    )?;
                    let post_only = r.u8()? == 1;
                    let price = r.i64()?;
                    let qty = r.i64()?;
                    Command::Submit(Order {
                        id,
                        owner,
                        side,
                        kind,
                        tif,
                        price,
                        qty,
                        post_only,
                        stp,
                    })
                }
                2 => Command::Cancel(r.u64()?),
                3 => Command::Modify(r.u64()?, r.i64()?, r.i64()?),
                k => return Err(FormatError(format!("unknown command kind {k}"))),
            };
            commands.push(cmd);
        }
        let events = r.u64()?;
        let trades = r.u64()?;
        let digest = r.u64()?;
        let event_hash = r.u64()?;
        let n = r.u32()? as usize;
        let checkpoints = (0..n).map(|_| r.u64()).collect::<Result<Vec<_>, _>>()?;
        if r.pos != data.len() {
            return Err(FormatError(format!(
                "{} bytes after the end",
                data.len() - r.pos
            )));
        }
        Ok(Flow {
            commands,
            expected: Expected {
                events,
                trades,
                digest,
                event_hash,
                checkpoints,
                checkpoint_every,
            },
        })
    }
}

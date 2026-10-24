//! A page's JSON, counted while it is written: the output never grows past
//! the budget, and a page that would is refused whole, never truncated.

use std::io;

use bi_audit::Event;
use serde::Serialize;

use super::PageError;

#[derive(Serialize)]
struct Body<'a> {
    events: &'a [Event],
    next_cursor: Option<&'a str>,
}

/// A buffer that refuses any write past `limit` bytes.
pub(super) struct Bounded {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl Bounded {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(64 * 1024)),
            limit,
            exceeded: false,
        }
    }
}

impl io::Write for Bounded {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.len() > self.limit - self.bytes.len() {
            self.exceeded = true;
            return Err(io::Error::other("page budget exceeded"));
        }
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// `{"events":[...],"next_cursor":...}` in at most `limit` bytes, UTF-8,
/// escapes and token included.
///
/// # Errors
/// [`PageError::BudgetExceeded`] when it would take more.
pub(super) fn page(
    events: &[Event],
    next_cursor: Option<&str>,
    limit: usize,
) -> Result<String, PageError> {
    let mut out = Bounded::new(limit);
    match serde_json::to_writer(
        &mut out,
        &Body {
            events,
            next_cursor,
        },
    ) {
        Ok(()) => String::from_utf8(out.bytes).map_err(|_| PageError::InvalidStoredEvent),
        Err(_) if out.exceeded => Err(PageError::BudgetExceeded),
        Err(_) => Err(PageError::InvalidStoredEvent),
    }
}

//! Anonymous reads of public HTTPS pages for the search map: no cookies,
//! credentials, referrer or page state, bounded in time, size and
//! concurrency, and the bytes are untrusted data. Diagnostics carry classes,
//! counts, sizes and durations, never an address, header or body.

mod content;
mod decode;
#[cfg(test)]
mod fixture;
pub mod guard;
mod limits;
mod platform;
mod probe;
mod reader;
mod route;
mod source;
#[cfg(test)]
mod tests;
mod types;

pub use limits::*;
pub use reader::{CounterSnapshot, PublicReader, ReadSession, USER_AGENT};
pub use route::PublicRoute;
pub use source::RouteSource;
pub use types::{
    Expect, PublicBody, ReadRefusal, ReadRequest, SizeLimit, StatusClass, StatusRefusal,
    TimeoutPhase, Timings,
};

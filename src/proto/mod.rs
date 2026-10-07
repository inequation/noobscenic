//! The shared protocol layer — doc/PLAN.md §11. Payloads arrive from both transports
//! (channel A `uploadEvents`/`response` bodies and channel B frames) and are routed by
//! `dispatch` here, so the channels only ever deal with framing.

pub mod dispatch;
pub mod info_type;

pub use dispatch::{Incoming, dispatch};

//! Wire `infoType` values — distinct from the 355-entry `EID_*` bus id space
//! (PROTOCOL.md §B). Only the ones phase 4 handles are named.

pub const HANDSHAKE: i64 = 10001;
pub const STATUS: i64 = 20001;
pub const MAP: i64 = 20002;
pub const EVENT: i64 = 20003;
pub const RECORD: i64 = 20004;
pub const ZONES_WRITE: i64 = 21003;
pub const ZONES_READ: i64 = 21004;
pub const CLEAN: i64 = 21005;
pub const PING: i64 = 21006;
pub const PATH: i64 = 21011;
pub const REMOTE_CONTROL: i64 = 21020;
pub const ZONE_CLEAN: i64 = 21023;

pub fn name(info_type: i64) -> Option<&'static str> {
    match info_type {
        HANDSHAKE => Some("handshake"),
        STATUS => Some("status"),
        MAP => Some("map"),
        EVENT => Some("event"),
        RECORD => Some("clean-record"),
        ZONES_WRITE => Some("zone-write"),
        ZONES_READ => Some("zone-read"),
        CLEAN => Some("clean"),
        PING => Some("ping"),
        PATH => Some("clean-path"),
        REMOTE_CONTROL => Some("remote-control"),
        ZONE_CLEAN => Some("zone-clean"),
        _ => None,
    }
}

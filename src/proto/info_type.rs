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
pub const GOODS_READ: i64 = 21015;
pub const GOODS_WRITE: i64 = 21016;
pub const REMOTE_CONTROL: i64 = 21020;
pub const SUCTION: i64 = 21022;
pub const ZONE_CLEAN: i64 = 21023;
pub const CONFIG: i64 = 21024;
pub const BACKUP_RESTORE: i64 = 21025;

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
        GOODS_READ => Some("goods-read"),
        GOODS_WRITE => Some("goods-write"),
        REMOTE_CONTROL => Some("remote-control"),
        SUCTION => Some("suction"),
        ZONE_CLEAN => Some("zone-clean"),
        CONFIG => Some("config"),
        BACKUP_RESTORE => Some("map-restore"),
        _ => None,
    }
}

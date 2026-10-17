//! Wire `infoType` values — distinct from the 355-entry `EID_*` bus id space
//! (PROTOCOL.md §B). Known types are named even when they only need a generic reply
//! handler.

pub const HANDSHAKE: i64 = 10001;
pub const STATUS: i64 = 20001;
pub const MAP: i64 = 20002;
pub const EVENT: i64 = 20003;
pub const RECORD: i64 = 20004;
pub const SCHEDULES_WRITE: i64 = 21001;
pub const SCHEDULES_READ: i64 = 21002;
pub const ZONES_WRITE: i64 = 21003;
pub const ZONES_READ: i64 = 21004;
pub const CLEAN: i64 = 21005;
pub const PING: i64 = 21006;
pub const DEVICE_ATTR: i64 = 21010;
pub const PATH: i64 = 21011;
pub const FIND_CHARGE: i64 = 21012;
pub const GOODS_READ: i64 = 21015;
pub const GOODS_WRITE: i64 = 21016;
pub const CLEAN_CONTROL: i64 = 21017;
pub const VERSION: i64 = 21018;
pub const NETWORK_INFO: i64 = 21019;
pub const REMOTE_CONTROL: i64 = 21020;
pub const OTA: i64 = 21021;
pub const SUCTION: i64 = 21022;
pub const ZONE_CLEAN: i64 = 21023;
pub const CONFIG: i64 = 21024;
pub const BACKUP_RESTORE: i64 = 21025;
pub const LOCAL_ENDPOINT: i64 = 21026;
pub const ECHO_QUERY: i64 = 21028;
pub const AREA_EDIT: i64 = 21030;
pub const BATCH: i64 = 30000;

pub fn name(info_type: i64) -> Option<&'static str> {
    match info_type {
        HANDSHAKE => Some("handshake"),
        STATUS => Some("status"),
        MAP => Some("map"),
        EVENT => Some("event"),
        RECORD => Some("clean-record"),
        SCHEDULES_WRITE => Some("schedules-write"),
        SCHEDULES_READ => Some("schedules-read"),
        ZONES_WRITE => Some("zone-write"),
        ZONES_READ => Some("zone-read"),
        CLEAN => Some("clean"),
        PING => Some("ping"),
        DEVICE_ATTR => Some("device-attr"),
        PATH => Some("clean-path"),
        FIND_CHARGE => Some("find-charge"),
        GOODS_READ => Some("goods-read"),
        GOODS_WRITE => Some("goods-write"),
        CLEAN_CONTROL => Some("clean-control"),
        VERSION => Some("version"),
        NETWORK_INFO => Some("network-info"),
        REMOTE_CONTROL => Some("remote-control"),
        OTA => Some("ota"),
        SUCTION => Some("suction"),
        ZONE_CLEAN => Some("zone-clean"),
        CONFIG => Some("config"),
        BACKUP_RESTORE => Some("map-restore"),
        LOCAL_ENDPOINT => Some("local-endpoint"),
        ECHO_QUERY => Some("echo-query"),
        AREA_EDIT => Some("area-edit"),
        BATCH => Some("batch"),
        _ => None,
    }
}

/// Whether a documented robot message is a command/query reply. These still reach
/// the catch-all storage path unless they have a dedicated table, but they are not
/// unknown protocol messages and must not raise the unknown-`infoType` warning.
pub(crate) fn is_known_reply(info_type: i64) -> bool {
    matches!(
        info_type,
        STATUS
            | SCHEDULES_WRITE
            | SCHEDULES_READ
            | ZONES_WRITE
            | ZONES_READ
            | CLEAN
            | DEVICE_ATTR
            | PATH
            | FIND_CHARGE
            | GOODS_READ
            | GOODS_WRITE
            | CLEAN_CONTROL
            | VERSION
            | NETWORK_INFO
            | OTA
            | SUCTION
            | ZONE_CLEAN
            | CONFIG
            | BACKUP_RESTORE
            | LOCAL_ENDPOINT
            | ECHO_QUERY
            | AREA_EDIT
            | BATCH
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_replies_are_known_and_no_reply_types_are_not() {
        for info_type in [
            STATUS,
            SCHEDULES_WRITE,
            SCHEDULES_READ,
            CLEAN,
            DEVICE_ATTR,
            FIND_CHARGE,
            CLEAN_CONTROL,
            VERSION,
            NETWORK_INFO,
            OTA,
            BACKUP_RESTORE,
            LOCAL_ENDPOINT,
            ECHO_QUERY,
            AREA_EDIT,
            BATCH,
        ] {
            assert!(
                is_known_reply(info_type),
                "{info_type} is a documented reply"
            );
        }

        for info_type in [REMOTE_CONTROL, 21027, 21029, 31337] {
            assert!(
                !is_known_reply(info_type),
                "{info_type} is unknown or documented as reply-less"
            );
        }
    }
}

//! The connection registry: which devices are online right now, and how to write
//! to them (doc/PLAN.md §3, §10.2).
//!
//! The robot opens the only socket it will accept cloud traffic on, so "send a
//! command to device X" means "look up X's writer here". Entries live exactly as
//! long as the connection that created them: the handshake inserts one, and a
//! guard on the connection task removes it no matter how the task ends.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::Value;
use tokio::sync::mpsc;

use crate::db::now_ms;

/// A live channel-B connection, as the rest of the server sees it.
#[derive(Clone, Debug)]
pub struct ConnHandle {
    pub conn_id: String,
    pub peer: String,
    pub connected_ms: i64,
    /// Channel-B frames for this connection, already framed by the writer task.
    pub tx: mpsc::Sender<Value>,
}

#[derive(Default)]
pub struct Registry {
    inner: Mutex<HashMap<String, ConnHandle>>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Bind `sn` to this connection. Returns the entry it replaced, if the device
    /// had an older connection that has not noticed yet.
    pub fn register(&self, sn: &str, handle: ConnHandle) -> Option<ConnHandle> {
        self.lock().insert(sn.to_string(), handle)
    }

    /// Remove `sn`'s entry, but only while it still belongs to `conn_id`.
    pub fn remove(&self, sn: &str, conn_id: &str) {
        let mut inner = self.lock();
        if inner
            .get(sn)
            .is_some_and(|handle| handle.conn_id == conn_id)
        {
            inner.remove(sn);
        }
    }

    pub fn get(&self, sn: &str) -> Option<ConnHandle> {
        self.lock().get(sn).cloned()
    }

    pub fn is_online(&self, sn: &str) -> bool {
        self.lock().contains_key(sn)
    }

    pub fn online(&self) -> Vec<ConnHandle> {
        self.lock().values().cloned().collect()
    }

    /// A poisoned lock must not take the gateway down; the map is only ever
    /// inserted into and removed from, so its contents stay sane either way.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, ConnHandle>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Owns one connection's registry entry; dropping it (cleanly, on error, or on a
/// panic unwinding the connection task) takes the entry out again.
pub struct Registration {
    registry: Arc<Registry>,
    sn: String,
    conn_id: String,
}

impl Registration {
    pub fn new(registry: Arc<Registry>, sn: String, conn_id: String) -> Registration {
        Registration {
            registry,
            sn,
            conn_id,
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.registry.remove(&self.sn, &self.conn_id);
    }
}

/// Build a handle for a connection; convenience so `conn.rs` does not have to
/// spell out the timestamp.
pub fn handle(conn_id: &str, peer: &str, tx: mpsc::Sender<Value>) -> ConnHandle {
    ConnHandle {
        conn_id: conn_id.to_string(),
        peer: peer.to_string(),
        connected_ms: now_ms(),
        tx,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel() -> mpsc::Sender<Value> {
        mpsc::channel(1).0
    }

    #[test]
    fn a_registered_device_is_online_until_its_connection_ends() {
        let registry = Arc::new(Registry::new());
        assert!(!registry.is_online("SN1"));

        registry.register("SN1", handle("b-000001", "10.0.0.5:1000", channel()));
        assert!(registry.is_online("SN1"));
        assert_eq!(registry.get("SN1").unwrap().conn_id, "b-000001");
        assert_eq!(registry.online().len(), 1);

        registry.remove("SN1", "b-000001");
        assert!(!registry.is_online("SN1"));
    }

    #[test]
    fn an_old_connection_cannot_remove_its_replacement() {
        let registry = Registry::new();
        registry.register("SN1", handle("b-000001", "10.0.0.5:1000", channel()));
        registry.register("SN1", handle("b-000002", "10.0.0.5:1001", channel()));

        // The stale task's cleanup runs late; it must not evict the live socket.
        registry.remove("SN1", "b-000001");
        assert_eq!(registry.get("SN1").unwrap().conn_id, "b-000002");
    }

    #[test]
    fn the_registration_guard_removes_the_entry_on_drop() {
        let registry = Arc::new(Registry::new());
        registry.register("SN1", handle("b-000001", "10.0.0.5:1000", channel()));
        {
            let _guard = Registration::new(registry.clone(), "SN1".into(), "b-000001".into());
            assert!(registry.is_online("SN1"));
        }
        assert!(
            !registry.is_online("SN1"),
            "the guard must clean up after itself"
        );
    }

    #[test]
    fn replacing_a_connection_reports_the_old_one() {
        let registry = Registry::new();
        assert!(
            registry
                .register("SN1", handle("b-000001", "10.0.0.5:1000", channel()))
                .is_none()
        );
        let replaced = registry
            .register("SN1", handle("b-000002", "10.0.0.5:1001", channel()))
            .expect("the older connection is handed back");
        assert_eq!(replaced.conn_id, "b-000001");
    }
}

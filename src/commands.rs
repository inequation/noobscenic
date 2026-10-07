//! The outbound command queue — doc/PLAN.md §10.3, §10.4.
//!
//! The `commands` table **is** the queue: the console, a one-shot CLI invocation and
//! plain `sqlite3` all enqueue rows, and this poller is the only thing that pushes
//! them — once a second, and only while the device is online. That removes any IPC
//! between the operator surface and the gateway. Rows older than
//! `gateway.command_ttl_secs` expire instead of firing hours later.

use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::watch;

use crate::AppState;
use crate::channel_b::codec;
use crate::channel_b::registry::ConnHandle;
use crate::db::now_ms;
use crate::db::queries::{self, QueuedCommand};

/// How often the queue is drained. Trivially cheap, and it makes the console feel
/// immediate without any in-process signalling.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// The `userId` every queued command's `dInfo` carries. The robot echoes it back in
/// `cleanPack/response`; nothing on the device side validates it.
pub const CLOUD_USER: &str = "noobscenic";

pub fn spawn(state: AppState, mut shutdown: watch::Receiver<()>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = ticker.tick() => drain_once(&state).await,
            }
        }
    });
}

/// One drain pass: expire stale rows, then push everything pending for the devices
/// that are online right now. Public so tests can drive it without the timer.
pub async fn drain_once(state: &AppState) {
    let ttl_ms = state.config.gateway.command_ttl_secs as i64 * 1000;
    match queries::expire_commands(&state.db, now_ms() - ttl_ms).await {
        Ok(0) => {}
        Ok(expired) => tracing::info!(expired, "command queue: expired stale pending commands"),
        Err(error) => tracing::error!(%error, "command queue: could not expire stale commands"),
    }

    let sns = match queries::pending_command_sns(&state.db).await {
        Ok(sns) => sns,
        Err(error) => {
            tracing::error!(%error, "command queue: could not list pending commands");
            return;
        }
    };
    for sn in sns {
        // Offline: the row waits for the device to dial in, or for the TTL.
        let Some(handle) = state.registry.get(&sn) else {
            continue;
        };
        let claimed = match queries::claim_commands(&state.db, &sn, now_ms()).await {
            Ok(claimed) => claimed,
            Err(error) => {
                tracing::error!(%error, %sn, "command queue: could not claim commands");
                continue;
            }
        };
        for command in claimed {
            push(state, &sn, &handle, command).await;
        }
    }
}

/// Frame one claimed command and hand it to the device's writer.
async fn push(state: &AppState, sn: &str, handle: &ConnHandle, command: QueuedCommand) {
    let data: Value = match serde_json::from_str(&command.payload) {
        Ok(data) => data,
        Err(error) => {
            fail(state, command.id, &format!("payload is not JSON: {error}")).await;
            return;
        }
    };

    let frame = if command.encrypt == 0 {
        // The default and the simplest correct path: an integer `encrypt`, the inner
        // message the dispatcher actually runs, and the `dInfo` its reply builder
        // needs (doc/PLAN.md §10.3).
        codec::envelope(
            0,
            codec::message(command.info_type, data, Some(reply_info())),
        )
    } else {
        if !state.config.gateway.encrypt_commands {
            fail(
                state,
                command.id,
                "encrypt:1 requested but gateway.encrypt_commands is off",
            )
            .await;
            return;
        }
        match encrypted_frame(state, sn, command.info_type, data).await {
            Ok(frame) => frame,
            Err(error) => {
                fail(state, command.id, &error).await;
                return;
            }
        }
    };

    tracing::info!(
        command_id = command.id,
        info_type = command.info_type,
        encrypt = command.encrypt,
        sn,
        "command pushed to device"
    );
    if handle
        .tx
        .send(crate::channel_b::registry::Outbound::command(
            command.id, frame,
        ))
        .await
        .is_err()
    {
        fail(
            state,
            command.id,
            "connection closed before the frame was written",
        )
        .await;
    }
}

/// `encrypt:1` — AES over the device's own session key (doc/PLAN.md §10.3).
async fn encrypted_frame(
    state: &AppState,
    sn: &str,
    info_type: i64,
    data: Value,
) -> std::result::Result<Value, String> {
    let key = match queries::latest_session_key(&state.db, sn).await {
        Ok(Some(key)) => key,
        Ok(None) => return Err("encrypt:1 but no live session key".to_string()),
        Err(error) => return Err(format!("session key lookup failed: {error}")),
    };
    let message = codec::message(info_type, data, Some(reply_info()));
    match crate::channel_b::crypto::encrypt_message(&message, &key) {
        Some(ciphertext) => Ok(codec::envelope(1, Value::String(ciphertext))),
        None => Err("session key is too short for AES-128".to_string()),
    }
}

/// The reply-correlation object the device's reply builder requires — both members
/// must be strings, or the device refuses to POST the reply.
fn reply_info() -> Value {
    json!({"ts": now_ms().to_string(), "userId": CLOUD_USER})
}

async fn fail(state: &AppState, id: i64, error: &str) {
    tracing::warn!(command_id = id, error, "command failed before it was sent");
    if let Err(error) = queries::fail_command(&state.db, id, error).await {
        tracing::error!(%error, command_id = id, "could not record the command failure");
    }
}

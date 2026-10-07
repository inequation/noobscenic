//! `POST /cleanPack/register` — adopt the device, mint its session and cookie.

use axum::extract::{Request, State};
use axum::response::Response;
use serde_json::json;

use crate::AppState;
use crate::channel_a::form::Form;
use crate::channel_a::handlers;
use crate::db::queries;
use crate::session;
use crate::wire::Recorded;

/// How many times to re-mint if a cookie collides with a live one.
const MINT_ATTEMPTS: usize = 5;

pub async fn register(State(state): State<AppState>, request: Request) -> Response {
    let recorded = request.extensions().get::<Recorded>().cloned();
    let trace = recorded
        .as_ref()
        .map(|r| r.reference.as_str())
        .unwrap_or("-");
    let body = axum::body::to_bytes(request.into_body(), crate::channel_a::MAX_BODY_BYTES)
        .await
        .unwrap_or_default();
    let form = Form::parse(&body);

    let Some(sn) = form
        .get("sn")
        .filter(|sn| !sn.is_empty())
        .map(str::to_string)
    else {
        tracing::warn!(
            trace,
            "register without an sn; answering code:0 without a session"
        );
        return handlers::ok(json!({}));
    };

    if !state.config.registration.accept_all
        && !queries::device_exists(&state.db, &sn)
            .await
            .unwrap_or(false)
    {
        tracing::warn!(sn = %sn, trace, "register from an unknown device; accept_all is off");
        return handlers::auth_failed();
    }

    // `sig` is RSA over "<sn>:<time>" under a key we do not have — stored, never
    // verified (doc/PLAN.md §9.2, PROTOCOL.md §B).
    if let Err(error) = queries::upsert_device_seen(&state.db, &sn, form.get("ld_sn")).await {
        tracing::error!(%error, sn = %sn, "could not upsert the device; registering anyway");
    }

    let sig = form.get("sig");
    for _ in 0..MINT_ATTEMPTS {
        let minted = session::mint();
        match queries::insert_session(&state.db, &sn, &minted.session_key, &minted.cookie, sig)
            .await
        {
            Ok(true) => {
                tracing::info!(sn = %sn, trace, "registered; session minted");
                return handlers::ok(json!({
                    "session": minted.session_key,
                    "cookies": minted.cookie,
                }));
            }
            Ok(false) => continue,
            Err(error) => {
                tracing::error!(%error, sn = %sn, "could not persist a session");
                break;
            }
        }
    }

    tracing::error!(sn = %sn, trace, "no session could be minted; answering code:0 without one");
    handlers::ok(json!({}))
}

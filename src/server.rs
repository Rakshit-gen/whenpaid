//! HTTP API, webhook receiver and the recompute worker.
//!
//! whenpaid never moves money. It reads what the provider reports and
//! explains it, so there is no outgoing transfer to protect with an outbox.

use crate::store::{self, Ingested};
use crate::webhook;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, SubsecRound, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::time::Duration;

/// Crash injection for tests: `WHENPAID_CRASH_AFTER=<point>` exits the process there.
pub fn crash_point(name: &str) -> bool {
    std::env::var("WHENPAID_CRASH_AFTER").is_ok_and(|v| v == name)
}

pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/webhooks/{business_id}", post(receive))
        .route("/v1/report", get(report))
        .route("/v1/payouts/{payout_id}/received", post(received))
        .with_state(pool)
}

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({ "error": msg.into() }))).into_response()
}

#[allow(clippy::result_large_err)] // the error is the HTTP response itself
fn header<'a>(h: &'a HeaderMap, name: &'static str) -> Result<&'a str, Response> {
    h.get(name)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, format!("missing {name} header")))
}

async fn receive(
    State(pool): State<PgPool>,
    Path(business_id): Path<String>,
    h: HeaderMap,
    body: Bytes,
) -> Response {
    let (id, ts, sig) = match (
        header(&h, "webhook-id"),
        header(&h, "webhook-timestamp"),
        header(&h, "webhook-signature"),
    ) {
        (Ok(a), Ok(b), Ok(c)) => (a, b, c),
        (Err(r), _, _) | (_, Err(r), _) | (_, _, Err(r)) => return r,
    };
    let tenant = match store::tenant_by_business(&pool, &business_id).await {
        Ok(Some(t)) => t,
        // Same answer as a bad signature, so the endpoint doesn't reveal which accounts exist.
        Ok(None) => return err(StatusCode::UNAUTHORIZED, "signature check failed"),
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let key = match webhook::decode_secret(&tenant.secret) {
        Ok(k) => k,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    if let Err(e) = webhook::verify(&key, id, ts, sig, &body, Utc::now().timestamp()) {
        return err(
            StatusCode::UNAUTHORIZED,
            format!("signature check failed: {e}"),
        );
    }
    let v: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("body is not JSON: {e}")),
    };
    // A correctly signed event for a different business is still not this tenant's data.
    if v["business_id"].as_str() != Some(tenant.business_id.as_str()) {
        return err(
            StatusCode::FORBIDDEN,
            "business_id in the body doesn't match this endpoint",
        );
    }
    match store::ingest_webhook(&pool, tenant.id, id, &v).await {
        Ok(Ingested::New) => (StatusCode::OK, Json(json!({ "stored": true }))).into_response(),
        Ok(Ingested::Duplicate) => (
            StatusCode::OK,
            Json(json!({ "stored": false, "duplicate": true })),
        )
            .into_response(),
        // A database error must not return 2xx, so the provider retries.
        Err(e) if e.starts_with("database") => err(StatusCode::SERVICE_UNAVAILABLE, e),
        // A malformed event will never succeed; accept it so retries stop, but keep nothing.
        Err(e) => (
            StatusCode::OK,
            Json(json!({ "stored": false, "ignored": e })),
        )
            .into_response(),
    }
}

#[allow(clippy::result_large_err)]
async fn auth(pool: &PgPool, h: &HeaderMap) -> Result<i64, Response> {
    let token = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| {
            err(
                StatusCode::UNAUTHORIZED,
                "send Authorization: Bearer <token>",
            )
        })?;
    match store::tenant_by_token(pool, token).await {
        Ok(Some(t)) => Ok(t),
        Ok(None) => Err(err(StatusCode::UNAUTHORIZED, "unknown token")),
        Err(e) => Err(err(StatusCode::SERVICE_UNAVAILABLE, e)),
    }
}

async fn report(State(pool): State<PgPool>, h: HeaderMap) -> Response {
    let tenant = match auth(&pool, &h).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    match store::latest_report(&pool, tenant).await {
        Ok(Some(r)) => Json(r).into_response(),
        Ok(None) => err(
            StatusCode::NOT_FOUND,
            "no report yet; import a ledger export and run the worker",
        ),
        Err(e) => err(StatusCode::SERVICE_UNAVAILABLE, e),
    }
}

#[derive(serde::Deserialize)]
struct ReceivedBody {
    at: DateTime<Utc>,
}

/// The merchant says the money reached their bank. The provider never knows this.
async fn received(
    State(pool): State<PgPool>,
    Path(payout_id): Path<String>,
    h: HeaderMap,
    Json(b): Json<ReceivedBody>,
) -> Response {
    let tenant = match auth(&pool, &h).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    match store::confirm_received(&pool, tenant, &payout_id, b.at).await {
        Ok(true) => Json(json!({ "confirmed": true })).into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "no such payout for this account"),
        Err(e) => err(StatusCode::SERVICE_UNAVAILABLE, e),
    }
}

pub async fn migrate(pool: PgPool) -> Result<(), String> {
    store::migrate(&pool).await?;
    println!("database is up to date");
    Ok(())
}

pub async fn serve(pool: PgPool) -> Result<(), String> {
    store::migrate(&pool).await?;
    let addr = std::env::var("WHENPAID_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("can't listen on {addr}: {e}"))?;
    eprintln!("whenpaid listening on {addr}");
    axum::serve(listener, router(pool))
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
        })
        .await
        .map_err(|e| e.to_string())
}

/// Process one job if there is one. Returns whether there was.
pub async fn work_once(pool: &PgPool) -> Result<bool, String> {
    let lease = std::env::var("WHENPAID_LEASE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);
    let Some(job) = store::claim_job(pool, lease).await? else {
        return Ok(false);
    };
    let as_of = Utc::now().trunc_subsecs(0);
    let result = async {
        let input = store::load_input(pool, job.tenant, as_of).await?;
        let r = crate::explain(&input)?;
        let mut body = serde_json::to_value(&r).unwrap();
        let hash = crate::input_hash(&input);
        body["input_sha256"] = hash.clone().into();
        Ok::<_, String>((hash, body))
    }
    .await;
    if crash_point("compute") {
        std::process::exit(3);
    }
    match result {
        Ok((hash, body)) => {
            store::finish_job(pool, &job, &hash, as_of, &body).await?;
        }
        Err(e) => store::fail_job(pool, &job, &e).await?,
    }
    Ok(true)
}

pub async fn worker(pool: PgPool) -> Result<(), String> {
    store::migrate(&pool).await?;
    let once = std::env::var("WHENPAID_WORKER_ONCE").is_ok();
    loop {
        match work_once(&pool).await {
            Ok(true) => continue,
            Ok(false) if once => return Ok(()),
            Ok(false) => tokio::time::sleep(Duration::from_secs(1)).await,
            Err(e) => {
                eprintln!("worker: {e}");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

pub async fn add_tenant(pool: PgPool, business_id: String, secret: String) -> Result<(), String> {
    store::migrate(&pool).await?;
    let token = store::add_tenant(&pool, &business_id, &secret).await?;
    println!(
        "registered {business_id}\napi token (shown once): {token}\nwebhook url: /webhooks/{business_id}"
    );
    Ok(())
}

pub async fn import(pool: PgPool, business_id: String, path: String) -> Result<(), String> {
    store::migrate(&pool).await?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    let input: crate::Input = serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
    if input
        .business_id
        .as_deref()
        .is_some_and(|b| b != business_id)
    {
        return Err(format!(
            "{path} is for {}, not {business_id}",
            input.business_id.unwrap()
        ));
    }
    let t = store::tenant_by_business(&pool, &business_id)
        .await?
        .ok_or_else(|| format!("{business_id} is not registered; run whenpaid tenant add first"))?;
    let n = store::import(&pool, t.id, &input).await?;
    println!(
        "{} new ledger entries, {} conflicting (kept aside, not applied), {} payouts",
        n.entries_new, n.entries_conflicting, n.payouts
    );
    Ok(())
}

//! Postgres tests: duplicate and reordered webhooks, concurrent workers,
//! crashes mid-job, and tenant isolation. Each test gets its own schema.
//!
//! Needs DATABASE_URL (any database you can create schemas in). Without it
//! these tests print a note and pass, so `cargo test` works offline; CI sets it.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use std::sync::atomic::{AtomicU32, Ordering};
use tower::ServiceExt;
use whenpaid::build::Builder;
use whenpaid::money::Currency;
use whenpaid::{server, store, webhook};

/// Test-only webhook secrets, built from a label so no secret-shaped string sits in the source.
fn secret(label: &str) -> String {
    use base64::Engine;
    format!(
        "whsec_{}",
        base64::engine::general_purpose::STANDARD.encode(label)
    )
}
static N: AtomicU32 = AtomicU32::new(0);

async fn fresh() -> Option<(PgPool, String)> {
    let Ok(base) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set; skipping database test");
        return None;
    };
    let schema = format!(
        "t_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    );
    let admin = PgPool::connect(&base)
        .await
        .expect("connect to DATABASE_URL");
    sqlx::query(&format!("drop schema if exists {schema} cascade"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("create schema {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let sep = if base.contains('?') { '&' } else { '?' };
    let url = format!("{base}{sep}options=-c%20search_path%3D{schema}");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(20)
        .connect(&url)
        .await
        .unwrap();
    store::migrate(&pool).await.unwrap();
    Some((pool, url))
}

async fn tenant(pool: &PgPool, business: &str) -> (i64, String) {
    let token = store::add_tenant(pool, business, &secret("tenant a"))
        .await
        .unwrap();
    (
        store::tenant_by_business(pool, business)
            .await
            .unwrap()
            .unwrap()
            .id,
        token,
    )
}

fn payout_event(business: &str, status: &str, ts: &str) -> Value {
    json!({
        "business_id": business, "type": format!("payout.{status}"), "timestamp": ts,
        "data": { "payload_type": "Payout", "payout_id": "pyt_1", "amount": 49_500, "currency": "USD", "fee": 500,
                  "created_at": "2026-11-18T00:00:04Z", "status": status, "business_id": business }
    })
}

fn signed(business: &str, id: &str, body: &Value, secret: &str) -> Request<Body> {
    let raw = serde_json::to_vec(body).unwrap();
    let ts = chrono::Utc::now().timestamp().to_string();
    let sig = webhook::sign(&webhook::decode_secret(secret).unwrap(), id, &ts, &raw);
    Request::post(format!("/webhooks/{business}"))
        .header("webhook-id", id)
        .header("webhook-timestamp", ts)
        .header("webhook-signature", sig)
        .header("content-type", "application/json")
        .body(Body::from(raw))
        .unwrap()
}

fn sample_input(business: &str) -> whenpaid::Input {
    let mut b = Builder::new();
    b.sale("pay_1", "2026-11-03T10:00:00Z", Currency::USD, 50_000, 0, 0);
    b.payout("pyt_1", "2026-11-18T00:00:04Z", Currency::USD, 49_500, 500);
    serde_json::from_value(
        json!({ "business_id": business, "as_of": "2026-11-24T00:00:00Z", "ledger": b.entries }),
    )
    .unwrap()
}

async fn count(pool: &PgPool, sql: &str, tenant: i64) -> i64 {
    sqlx::query(sql)
        .bind(tenant)
        .fetch_one(pool)
        .await
        .unwrap()
        .get(0)
}

#[tokio::test]
async fn duplicate_webhook_is_stored_once() {
    let Some((pool, _)) = fresh().await else {
        return;
    };
    let (t, _) = tenant(&pool, "bus_dup").await;
    let body = payout_event("bus_dup", "success", "2026-11-19T10:00:00Z");
    let mut tasks = Vec::new();
    for _ in 0..25 {
        let (pool, body) = (pool.clone(), body.clone());
        tasks.push(tokio::spawn(async move {
            store::ingest_webhook(&pool, t, "msg_same", &body)
                .await
                .unwrap()
        }));
    }
    let mut new = 0;
    for task in tasks {
        if task.await.unwrap() == store::Ingested::New {
            new += 1;
        }
    }
    assert_eq!(new, 1);
    assert_eq!(
        count(&pool, "select count(*) from inbox where tenant_id = $1", t).await,
        1
    );
    let rec: Value = sqlx::query("select record from payouts where tenant_id = $1")
        .bind(t)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get(0);
    assert_eq!(rec["events"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn late_older_payout_event_does_not_win() {
    let Some((pool, _)) = fresh().await else {
        return;
    };
    let (t, _) = tenant(&pool, "bus_order").await;
    store::ingest_webhook(
        &pool,
        t,
        "m2",
        &payout_event("bus_order", "failed", "2026-11-21T10:00:00Z"),
    )
    .await
    .unwrap();
    store::ingest_webhook(
        &pool,
        t,
        "m1",
        &payout_event("bus_order", "success", "2026-11-19T10:00:00Z"),
    )
    .await
    .unwrap();
    store::ingest_webhook(
        &pool,
        t,
        "m0",
        &payout_event("bus_order", "in_progress", "2026-11-18T01:00:00Z"),
    )
    .await
    .unwrap();
    let row = sqlx::query("select status, record from payouts where tenant_id = $1")
        .bind(t)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>(0), "failed");
    assert_eq!(
        row.get::<Value, _>(1)["events"].as_array().unwrap().len(),
        3,
        "older events are kept as history"
    );
}

#[tokio::test]
async fn webhook_endpoint_checks_signature_and_tenant() {
    let Some((pool, _)) = fresh().await else {
        return;
    };
    tenant(&pool, "bus_a").await;
    store::add_tenant(&pool, "bus_b", &secret("tenant b"))
        .await
        .unwrap();
    let app = server::router(pool.clone());
    let ok = payout_event("bus_a", "success", "2026-11-19T10:00:00Z");
    let r = app
        .clone()
        .oneshot(signed("bus_a", "msg_1", &ok, &secret("tenant a")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let r = app
        .clone()
        .oneshot(signed("bus_a", "msg_1", &ok, &secret("tenant a")))
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::OK,
        "duplicates still get 2xx so the sender stops retrying"
    );
    // Signed with bus_b's secret but sent to bus_a.
    let r = app
        .clone()
        .oneshot(signed("bus_a", "msg_2", &ok, &secret("tenant b")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    // Correctly signed for bus_a, but the body claims bus_b.
    let r = app
        .clone()
        .oneshot(signed(
            "bus_a",
            "msg_3",
            &payout_event("bus_b", "success", "2026-11-19T10:00:00Z"),
            &secret("tenant a"),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = app
        .clone()
        .oneshot(signed("bus_nobody", "msg_4", &ok, &secret("tenant a")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn tenants_only_see_their_own_report() {
    let Some((pool, _)) = fresh().await else {
        return;
    };
    let (ta, token_a) = tenant(&pool, "bus_a").await;
    let (tb, _) = tenant(&pool, "bus_b").await;
    store::import(&pool, ta, &sample_input("bus_a"))
        .await
        .unwrap();
    while server::work_once(&pool).await.unwrap() {}
    assert_eq!(
        count(
            &pool,
            "select count(*) from reports where tenant_id = $1",
            tb
        )
        .await,
        0
    );
    let app = server::router(pool.clone());
    let r = app
        .clone()
        .oneshot(
            Request::get("/v1/report")
                .header("authorization", format!("Bearer {token_a}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 1 << 24).await.unwrap())
            .unwrap();
    assert_eq!(body["business_id"], "bus_a");
    let r = app
        .oneshot(
            Request::get("/v1/report")
                .header("authorization", "Bearer wp_wrong")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn concurrent_workers_compute_each_version_once() {
    let Some((pool, _)) = fresh().await else {
        return;
    };
    let mut tenants = Vec::new();
    for i in 0..10 {
        let b = format!("bus_{i}");
        let (t, _) = tenant(&pool, &b).await;
        store::import(&pool, t, &sample_input(&b)).await.unwrap();
        tenants.push(t);
    }
    let mut workers = Vec::new();
    for _ in 0..8 {
        let pool = pool.clone();
        workers.push(tokio::spawn(async move {
            let mut done = 0;
            while server::work_once(&pool).await.unwrap() {
                done += 1;
            }
            done
        }));
    }
    let mut total = 0;
    for w in workers {
        total += w.await.unwrap();
    }
    assert_eq!(total, 10, "each job claimed exactly once");
    for t in tenants {
        assert_eq!(
            count(
                &pool,
                "select count(*) from reports where tenant_id = $1",
                t
            )
            .await,
            1
        );
        assert_eq!(
            count(&pool, "select count(*) from jobs where tenant_id = $1", t).await,
            0
        );
    }
}

#[tokio::test]
async fn new_data_during_a_job_keeps_the_job() {
    let Some((pool, _)) = fresh().await else {
        return;
    };
    let (t, _) = tenant(&pool, "bus_race").await;
    store::import(&pool, t, &sample_input("bus_race"))
        .await
        .unwrap();
    let job = store::claim_job(&pool, 60).await.unwrap().unwrap();
    // A webhook lands while the worker is busy.
    store::ingest_webhook(
        &pool,
        t,
        "m1",
        &payout_event("bus_race", "success", "2026-11-19T10:00:00Z"),
    )
    .await
    .unwrap();
    let input = store::load_input(&pool, t, chrono::Utc::now())
        .await
        .unwrap();
    let r = serde_json::to_value(whenpaid::explain(&input).unwrap()).unwrap();
    let cleared = store::finish_job(&pool, &job, &whenpaid::input_hash(&input), input.as_of, &r)
        .await
        .unwrap();
    assert!(!cleared, "version moved, so the job must stay");
    assert!(server::work_once(&pool).await.unwrap());
    assert_eq!(
        count(&pool, "select count(*) from jobs where tenant_id = $1", t).await,
        0
    );
}

#[tokio::test]
async fn crash_mid_job_loses_nothing() {
    let Some((pool, url)) = fresh().await else {
        return;
    };
    let (t, _) = tenant(&pool, "bus_crash").await;
    store::import(&pool, t, &sample_input("bus_crash"))
        .await
        .unwrap();
    for point in ["compute", "after_report"] {
        let status = tokio::process::Command::new(env!("CARGO_BIN_EXE_whenpaid"))
            .arg("worker")
            .env("DATABASE_URL", &url)
            .env("WHENPAID_WORKER_ONCE", "1")
            .env("WHENPAID_LEASE_SECS", "1")
            .env("WHENPAID_CRASH_AFTER", point)
            .status()
            .await
            .unwrap();
        assert_eq!(
            status.code(),
            Some(3),
            "worker should have crashed at {point}"
        );
        assert_eq!(
            count(
                &pool,
                "select count(*) from reports where tenant_id = $1",
                t
            )
            .await,
            0,
            "crash at {point} left a report behind"
        );
        assert_eq!(
            count(&pool, "select count(*) from jobs where tenant_id = $1", t).await,
            1,
            "crash at {point} lost the job"
        );
        tokio::time::sleep(std::time::Duration::from_millis(1_200)).await; // let the lease run out
    }
    let status = tokio::process::Command::new(env!("CARGO_BIN_EXE_whenpaid"))
        .arg("worker")
        .env("DATABASE_URL", &url)
        .env("WHENPAID_WORKER_ONCE", "1")
        .status()
        .await
        .unwrap();
    assert!(status.success());
    assert_eq!(
        count(
            &pool,
            "select count(*) from reports where tenant_id = $1",
            t
        )
        .await,
        1
    );
    assert_eq!(
        count(&pool, "select count(*) from jobs where tenant_id = $1", t).await,
        0
    );
}

#[tokio::test]
async fn reimport_is_idempotent_and_changed_rows_are_kept_aside() {
    let Some((pool, _)) = fresh().await else {
        return;
    };
    let (t, _) = tenant(&pool, "bus_imp").await;
    let input = sample_input("bus_imp");
    let first = store::import(&pool, t, &input).await.unwrap();
    assert_eq!(first.entries_new, input.ledger.len() as u64);
    let again = store::import(&pool, t, &input).await.unwrap();
    assert_eq!((again.entries_new, again.entries_conflicting), (0, 0));
    let mut changed = input.clone();
    changed.ledger[0].amount += 1;
    let third = store::import(&pool, t, &changed).await.unwrap();
    assert_eq!((third.entries_new, third.entries_conflicting), (0, 1));
    let stored: Value =
        sqlx::query("select body from entries where tenant_id = $1 and id = 'pay_1'")
            .bind(t)
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
    assert_eq!(
        stored["amount"], 50_000,
        "the original row is never overwritten"
    );
}

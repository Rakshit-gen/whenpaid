//! Postgres storage. Every statement is scoped by tenant_id.

use crate::engine::{Input, PayoutRecord, PayoutStatus, StatusEvent};
use crate::ledger::LedgerEntry;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

pub type Result<T> = std::result::Result<T, String>;

fn e(err: sqlx::Error) -> String {
    format!("database: {err}")
}

pub async fn migrate(pool: &PgPool) -> Result<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(|err| format!("migration failed: {err}"))
}

pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn random_token() -> Result<String> {
    use std::io::Read;
    let mut b = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .map_err(|err| format!("no randomness: {err}"))?;
    Ok(format!("wp_{}", hex::encode(b)))
}

/// Register an account. Returns the API token; only its hash is stored.
pub async fn add_tenant(pool: &PgPool, business_id: &str, secret: &str) -> Result<String> {
    crate::webhook::decode_secret(secret).map_err(|r| r.to_string())?;
    let token = random_token()?;
    sqlx::query(
        "insert into tenants (business_id, webhook_secret, api_token_sha256) values ($1, $2, $3)",
    )
    .bind(business_id)
    .bind(secret)
    .bind(hash_token(&token))
    .execute(pool)
    .await
    .map_err(|err| {
        if err
            .as_database_error()
            .is_some_and(|d| d.is_unique_violation())
        {
            format!("{business_id} is already registered")
        } else {
            e(err)
        }
    })?;
    Ok(token)
}

pub struct Tenant {
    pub id: i64,
    pub business_id: String,
    pub secret: String,
}

pub async fn tenant_by_business(pool: &PgPool, business_id: &str) -> Result<Option<Tenant>> {
    let row =
        sqlx::query("select id, business_id, webhook_secret from tenants where business_id = $1")
            .bind(business_id)
            .fetch_optional(pool)
            .await
            .map_err(e)?;
    Ok(row.map(|r| Tenant {
        id: r.get(0),
        business_id: r.get(1),
        secret: r.get(2),
    }))
}

pub async fn tenant_by_token(pool: &PgPool, token: &str) -> Result<Option<i64>> {
    let row = sqlx::query("select id from tenants where api_token_sha256 = $1")
        .bind(hash_token(token))
        .fetch_optional(pool)
        .await
        .map_err(e)?;
    Ok(row.map(|r| r.get(0)))
}

async fn mark_dirty(tx: &mut sqlx::PgConnection, tenant: i64) -> Result<()> {
    sqlx::query("insert into jobs (tenant_id) values ($1) on conflict (tenant_id) do update set version = jobs.version + 1, due_at = least(jobs.due_at, now())")
        .bind(tenant)
        .execute(tx)
        .await
        .map_err(e)?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Ingested {
    New,
    Duplicate,
}

/// Store a verified webhook once and apply it. Duplicates (same webhook-id) change nothing (WH-DEDUP).
pub async fn ingest_webhook(
    pool: &PgPool,
    tenant: i64,
    webhook_id: &str,
    body: &Value,
) -> Result<Ingested> {
    let event_type = body["type"].as_str().ok_or("webhook body has no type")?;
    let ts: DateTime<Utc> = body["timestamp"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or("webhook body has no valid timestamp")?;
    let mut tx = pool.begin().await.map_err(e)?;
    let inserted = sqlx::query("insert into inbox (tenant_id, webhook_id, event_type, event_ts, body) values ($1, $2, $3, $4, $5) on conflict do nothing")
        .bind(tenant)
        .bind(webhook_id)
        .bind(event_type)
        .bind(ts)
        .bind(body)
        .execute(&mut *tx)
        .await
        .map_err(e)?
        .rows_affected();
    if inserted == 0 {
        return Ok(Ingested::Duplicate);
    }
    if event_type.starts_with("payout.") {
        apply_payout_event(&mut tx, tenant, &body["data"], ts, webhook_id).await?;
        mark_dirty(&mut tx, tenant).await?;
    }
    tx.commit().await.map_err(e)?;
    Ok(Ingested::New)
}

/// Upsert a payout from a webhook. The event joins the history either way; the
/// current status changes only if this event is newer than the last one applied (WH-PAYOUT-ORDER).
async fn apply_payout_event(
    tx: &mut sqlx::PgConnection,
    tenant: i64,
    d: &Value,
    ts: DateTime<Utc>,
    webhook_id: &str,
) -> Result<()> {
    let status: PayoutStatus = serde_json::from_value(d["status"].clone())
        .map_err(|_| format!("unknown payout status {}", d["status"]))?;
    let rec = PayoutRecord {
        payout_id: d["payout_id"]
            .as_str()
            .ok_or("payout event has no payout_id")?
            .to_string(),
        currency: serde_json::from_value(d["currency"].clone())
            .map_err(|err| format!("payout currency: {err}"))?,
        amount: d["amount"]
            .as_i64()
            .ok_or("payout amount must be an integer in minor units")?,
        fee: d["fee"].as_i64(),
        created_at: d["created_at"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(ts),
        events: vec![StatusEvent {
            status,
            timestamp: ts,
        }],
        bank_confirmed_at: None,
        remarks: d["remarks"].as_str().map(String::from),
    };
    let status_text = serde_json::to_value(status).unwrap();
    let _ = webhook_id;
    upsert_payout(tx, tenant, &rec, status_text.as_str(), Some(ts)).await
}

async fn upsert_payout(
    tx: &mut sqlx::PgConnection,
    tenant: i64,
    rec: &PayoutRecord,
    status: Option<&str>,
    status_at: Option<DateTime<Utc>>,
) -> Result<()> {
    sqlx::query(
        "insert into payouts (tenant_id, payout_id, record, status, status_at) values ($1, $2, $3, $4, $5)
         on conflict (tenant_id, payout_id) do update set
           record = (case when excluded.status_at is not null and (payouts.status_at is null or excluded.status_at > payouts.status_at)
                          then excluded.record else payouts.record end)
                    || jsonb_build_object(
                         'events', (select coalesce(jsonb_agg(distinct ev), '[]') from jsonb_array_elements((payouts.record->'events') || (excluded.record->'events')) ev),
                         'bank_confirmed_at', coalesce(payouts.record->'bank_confirmed_at', excluded.record->'bank_confirmed_at')),
           status = case when excluded.status_at is not null and (payouts.status_at is null or excluded.status_at > payouts.status_at) then excluded.status else payouts.status end,
           status_at = greatest(payouts.status_at, excluded.status_at)",
    )
    .bind(tenant)
    .bind(&rec.payout_id)
    .bind(serde_json::to_value(rec).unwrap())
    .bind(status)
    .bind(status_at)
    .execute(tx)
    .await
    .map_err(e)?;
    Ok(())
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Imported {
    pub entries_new: u64,
    pub entries_conflicting: u64,
    pub payouts: u64,
}

/// Backfill from an export. Rows we already hold are skipped; rows that
/// changed are recorded as conflicts and not applied. Settings, rates and
/// account state in the file replace what is stored.
pub async fn import(pool: &PgPool, tenant: i64, input: &Input) -> Result<Imported> {
    let mut out = Imported::default();
    let mut tx = pool.begin().await.map_err(e)?;
    for chunk in input.ledger.chunks(5_000) {
        let rows = serde_json::to_value(chunk).unwrap();
        out.entries_new += sqlx::query(
            "insert into entries (tenant_id, id, created_at, body)
             select $1, x->>'id', (x->>'created_at')::timestamptz, x from jsonb_array_elements($2) x
             on conflict do nothing",
        )
        .bind(tenant)
        .bind(&rows)
        .execute(&mut *tx)
        .await
        .map_err(e)?
        .rows_affected();
        out.entries_conflicting += sqlx::query(
            "insert into conflicts (tenant_id, kind, ref, existing, incoming)
             select $1, 'ledger_entry', en.id, en.body, x from jsonb_array_elements($2) x
             join entries en on en.tenant_id = $1 and en.id = x->>'id'
             where en.body <> x
               and not exists (select 1 from conflicts c where c.tenant_id = $1 and c.ref = en.id and c.incoming = x)",
        )
        .bind(tenant)
        .bind(&rows)
        .execute(&mut *tx)
        .await
        .map_err(e)?
        .rows_affected();
    }
    for p in &input.payouts {
        let last = p.events.iter().max_by_key(|x| x.timestamp);
        let status = last.map(|x| serde_json::to_value(x.status).unwrap());
        upsert_payout(
            &mut tx,
            tenant,
            p,
            status.as_ref().and_then(|v| v.as_str()),
            last.map(|x| x.timestamp),
        )
        .await?;
        out.payouts += 1;
    }
    sqlx::query("update tenants set settings = $2, params = $3, rates = $4, forms = $5, restrictions = $6 where id = $1")
        .bind(tenant)
        .bind(serde_json::to_value(&input.settings).unwrap())
        .bind(serde_json::to_value(&input.params).unwrap())
        .bind(serde_json::to_value(&input.rates).unwrap())
        .bind(serde_json::to_value(&input.forms).unwrap())
        .bind(serde_json::to_value(&input.restrictions).unwrap())
        .execute(&mut *tx)
        .await
        .map_err(e)?;
    mark_dirty(&mut tx, tenant).await?;
    tx.commit().await.map_err(e)?;
    Ok(out)
}

pub async fn confirm_received(
    pool: &PgPool,
    tenant: i64,
    payout_id: &str,
    at: DateTime<Utc>,
) -> Result<bool> {
    let mut tx = pool.begin().await.map_err(e)?;
    let n = sqlx::query("update payouts set record = jsonb_set(record, '{bank_confirmed_at}', to_jsonb($3::timestamptz)) where tenant_id = $1 and payout_id = $2")
        .bind(tenant)
        .bind(payout_id)
        .bind(at)
        .execute(&mut *tx)
        .await
        .map_err(e)?
        .rows_affected();
    if n > 0 {
        mark_dirty(&mut tx, tenant).await?;
    }
    tx.commit().await.map_err(e)?;
    Ok(n > 0)
}

/// Read everything for one tenant in one snapshot.
pub async fn load_input(pool: &PgPool, tenant: i64, as_of: DateTime<Utc>) -> Result<Input> {
    let mut tx = pool.begin().await.map_err(e)?;
    sqlx::query("set transaction isolation level repeatable read read only")
        .execute(&mut *tx)
        .await
        .map_err(e)?;
    let t = sqlx::query("select business_id, settings, params, rates, forms, restrictions from tenants where id = $1").bind(tenant).fetch_one(&mut *tx).await.map_err(e)?;
    let parse = |v: Value, what: &str| -> Result<Value> {
        if v.is_null() {
            Err(format!("tenant {tenant} has no {what}"))
        } else {
            Ok(v)
        }
    };
    let ledger: Vec<LedgerEntry> =
        sqlx::query("select body from entries where tenant_id = $1 order by created_at, id")
            .bind(tenant)
            .fetch_all(&mut *tx)
            .await
            .map_err(e)?
            .into_iter()
            .map(|r| {
                serde_json::from_value(r.get::<Value, _>(0))
                    .map_err(|err| format!("stored entry: {err}"))
            })
            .collect::<Result<_>>()?;
    let payouts: Vec<PayoutRecord> =
        sqlx::query("select record from payouts where tenant_id = $1 order by payout_id")
            .bind(tenant)
            .fetch_all(&mut *tx)
            .await
            .map_err(e)?
            .into_iter()
            .map(|r| {
                serde_json::from_value(r.get::<Value, _>(0))
                    .map_err(|err| format!("stored payout: {err}"))
            })
            .collect::<Result<_>>()?;
    tx.commit().await.map_err(e)?;
    let input = json!({
        "business_id": t.get::<String, _>(0),
        "as_of": as_of,
        "settings": parse(t.get(1), "settings")?,
        "params": parse(t.get(2), "params")?,
        "rates": parse(t.get(3), "rates")?,
        "forms": parse(t.get(4), "forms")?,
        "restrictions": parse(t.get(5), "restrictions")?,
        "ledger": ledger,
        "payouts": payouts,
    });
    serde_json::from_value(input).map_err(|err| format!("stored account data: {err}"))
}

pub struct Job {
    pub tenant: i64,
    pub version: i64,
}

/// Lease the oldest due job. Other workers skip it while the lease lasts; if
/// this worker dies, the lease runs out and someone else picks it up.
pub async fn claim_job(pool: &PgPool, lease_secs: i64) -> Result<Option<Job>> {
    let row = sqlx::query(
        "update jobs set leased_until = now() + make_interval(secs => $1), attempts = attempts + 1
         where tenant_id = (
           select tenant_id from jobs
           where due_at <= now() and (leased_until is null or leased_until < now())
           order by due_at
           for update skip locked
           limit 1)
         returning tenant_id, version",
    )
    .bind(lease_secs as f64)
    .fetch_optional(pool)
    .await
    .map_err(e)?;
    Ok(row.map(|r| Job {
        tenant: r.get(0),
        version: r.get(1),
    }))
}

/// Save the report and clear the job in one transaction. If new data arrived
/// while we computed, the version moved: the job stays and is picked up again.
pub async fn finish_job(
    pool: &PgPool,
    job: &Job,
    input_sha256: &str,
    as_of: DateTime<Utc>,
    body: &Value,
) -> Result<bool> {
    let mut tx = pool.begin().await.map_err(e)?;
    sqlx::query("insert into reports (tenant_id, input_sha256, as_of, body) values ($1, $2, $3, $4) on conflict do nothing")
        .bind(job.tenant)
        .bind(input_sha256)
        .bind(as_of)
        .bind(body)
        .execute(&mut *tx)
        .await
        .map_err(e)?;
    if crate::server::crash_point("after_report") {
        std::process::exit(3);
    }
    let cleared = sqlx::query("delete from jobs where tenant_id = $1 and version = $2")
        .bind(job.tenant)
        .bind(job.version)
        .execute(&mut *tx)
        .await
        .map_err(e)?
        .rows_affected()
        == 1;
    if !cleared {
        sqlx::query("update jobs set leased_until = null, attempts = 0 where tenant_id = $1")
            .bind(job.tenant)
            .execute(&mut *tx)
            .await
            .map_err(e)?;
    }
    tx.commit().await.map_err(e)?;
    Ok(cleared)
}

pub async fn fail_job(pool: &PgPool, job: &Job, error: &str) -> Result<()> {
    // Back off: 30s, 60s, 120s ... capped at an hour.
    sqlx::query("update jobs set leased_until = null, last_error = $2, due_at = now() + make_interval(secs => least(3600, 30 * power(2, greatest(attempts - 1, 0)))) where tenant_id = $1")
        .bind(job.tenant)
        .bind(error)
        .execute(pool)
        .await
        .map_err(e)?;
    Ok(())
}

pub async fn latest_report(pool: &PgPool, tenant: i64) -> Result<Option<Value>> {
    let row = sqlx::query("select body from reports where tenant_id = $1 order by as_of desc, created_at desc limit 1").bind(tenant).fetch_optional(pool).await.map_err(e)?;
    Ok(row.map(|r| r.get(0)))
}

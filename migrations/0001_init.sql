-- One row per merchant account. Every other table is keyed by tenant_id first,
-- and every query filters on it.
create table tenants (
    id                bigserial primary key,
    business_id       text not null unique,
    webhook_secret    text not null,
    api_token_sha256  text not null unique,
    settings          jsonb not null default '{}',
    params            jsonb not null default '{}',
    rates             jsonb not null default '[]',
    forms             jsonb not null default '[]',
    restrictions      jsonb not null default '[]',
    created_at        timestamptz not null default now()
);

-- Every verified webhook, once. The primary key is the dedup (WH-DEDUP).
create table inbox (
    tenant_id    bigint not null references tenants(id),
    webhook_id   text not null,
    event_type   text not null,
    event_ts     timestamptz not null,
    body         jsonb not null,
    received_at  timestamptz not null default now(),
    primary key (tenant_id, webhook_id)
);

-- Ledger rows exactly as exported. Never updated; a changed row goes to conflicts.
create table entries (
    tenant_id   bigint not null references tenants(id),
    id          text not null,
    created_at  timestamptz not null,
    body        jsonb not null,
    primary key (tenant_id, id)
);

create table payouts (
    tenant_id   bigint not null references tenants(id),
    payout_id   text not null,
    record      jsonb not null,
    status      text,
    status_at   timestamptz,
    primary key (tenant_id, payout_id)
);

-- Input that contradicts what we already hold. Kept for a human, never applied.
create table conflicts (
    id          bigserial primary key,
    tenant_id   bigint not null references tenants(id),
    kind        text not null,
    ref         text not null,
    existing    jsonb,
    incoming    jsonb not null,
    at          timestamptz not null default now()
);

-- One pending recompute per tenant. `version` goes up on every new piece of
-- data, so a worker only clears the job if nothing arrived while it worked.
create table jobs (
    tenant_id     bigint primary key references tenants(id),
    version       bigint not null default 1,
    due_at        timestamptz not null default now(),
    leased_until  timestamptz,
    attempts      int not null default 0,
    last_error    text
);
create index jobs_due on jobs (due_at);

create table reports (
    tenant_id     bigint not null references tenants(id),
    input_sha256  text not null,
    as_of         timestamptz not null,
    body          jsonb not null,
    created_at    timestamptz not null default now(),
    primary key (tenant_id, input_sha256)
);
create index reports_latest on reports (tenant_id, as_of desc);

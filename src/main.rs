use std::process::exit;
use whenpaid::{Input, policy, report};

const USAGE: &str = "usage:
  whenpaid explain <input.json> [--all] [--entries] [--json]
      Explain every payout run in the input: what was counted, why, which rule,
      what was or will be sent, and what is blocking anything.
  whenpaid rules [--markdown]
      Print every rule with its source page and the tests that cover it.
  whenpaid serve
      Run the webhook receiver and API (needs DATABASE_URL).
  whenpaid worker
      Recompute reports for accounts with new data (needs DATABASE_URL).
  whenpaid tenant add <business_id> <webhook_secret>
      Register an account; prints its API token once.
  whenpaid import <business_id> <input.json>
      Backfill ledger entries and payouts from an export.
  whenpaid migrate
      Create or update the database tables.

Exit status: 0 clean, 1 error, 2 the report has discrepancies or findings.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match a.as_slice() {
        ["explain", path, flags @ ..] => explain(path, flags),
        ["rules", rest @ ..] => {
            if rest.contains(&"--markdown") {
                print!("{}", policy::matrix_markdown());
            } else {
                for r in policy::RULES {
                    println!(
                        "{:<24} {:<14} {}\n{:<39} {}\n",
                        r.id,
                        format!("{:?}", r.basis),
                        r.says,
                        "",
                        source_url(r.source)
                    );
                }
            }
        }
        ["serve"] => db(whenpaid_server::serve),
        ["worker"] => db(whenpaid_server::worker),
        ["tenant", "add", business, secret] => {
            db(|pool| whenpaid_server::add_tenant(pool, business.to_string(), secret.to_string()))
        }
        ["import", business, path] => {
            db(|pool| whenpaid_server::import(pool, business.to_string(), path.to_string()))
        }
        ["migrate"] => db(whenpaid_server::migrate),
        ["--version"] => println!("whenpaid {}", env!("CARGO_PKG_VERSION")),
        ["-h"] | ["--help"] | ["help"] => println!("{USAGE}"),
        _ => fail(USAGE),
    }
}

fn source_url(key: &str) -> &'static str {
    policy::SOURCES
        .iter()
        .find(|s| s.key == key)
        .map_or("", |s| s.url)
}

fn explain(path: &str, flags: &[&str]) {
    for f in flags {
        if !["--all", "--entries", "--json"].contains(f) {
            fail(&format!("unknown flag {f}\n\n{USAGE}"));
        }
    }
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| fail(&format!("{path}: {e}")));
    let input: Input =
        serde_json::from_str(&text).unwrap_or_else(|e| fail(&format!("{path}: {e}")));
    let r = whenpaid::explain(&input).unwrap_or_else(|e| fail(&e));
    if flags.contains(&"--json") {
        let mut v = serde_json::to_value(&r).unwrap();
        v["input_sha256"] = whenpaid::input_hash(&input).into();
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
    } else {
        print!(
            "{}",
            report::render(
                &r,
                &report::Options {
                    all: flags.contains(&"--all"),
                    entries: flags.contains(&"--entries")
                }
            )
        );
    }
    if r.discrepancies.iter().any(|d| d.is_error())
        || r.findings
            .iter()
            .any(|f| !matches!(f, whenpaid::ledger::Finding::Duplicate { .. }))
    {
        exit(2);
    }
}

use whenpaid::server as whenpaid_server;

fn db<F, Fut>(f: F)
where
    F: FnOnce(sqlx::PgPool) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| fail("set DATABASE_URL, for example postgres://localhost/whenpaid"));
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(16)
            .connect(&url)
            .await
            .unwrap_or_else(|e| fail(&format!("can't connect to the database: {e}")));
        if let Err(e) = f(pool).await {
            fail(&e);
        }
    });
}

fn fail(msg: &str) -> ! {
    eprintln!("whenpaid: {msg}");
    exit(1)
}

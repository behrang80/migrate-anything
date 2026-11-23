//! Demo mode: a synthetic legacy CRM export and an empty destination, so a first migration can be
//! run end to end without any credentials or real data. Everything stays in the workspace folder.

use std::fmt::Write as _;
use std::path::Path;

use ma_core::{Actor, App};
use ma_types::{ConnectionConfig, Environment, MaResult};
use serde_json::{Value, json};

pub const SOURCE: &str = "demo-legacy-crm";
pub const DESTINATION: &str = "demo-new-system";

const ORGS: usize = 2_000;
const CONTACTS: usize = 24_000;
const OPPORTUNITIES: usize = 16_000;
const ACTIVITIES: usize = 48_000;

/// Small deterministic generator (SplitMix64) so the demo data is identical on every machine.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len() as u64) as usize]
    }
}

fn write_source(dir: &Path) -> MaResult<()> {
    std::fs::create_dir_all(dir)?;
    let mut r = Rng(2026);
    let industries = ["Manufacturing", "Retail", "Software", "Healthcare", "Logistics", "Energy", "Education"];
    let first = ["Ann", "Bo", "Zoë", "Søren", "Mía", "Li", "Omar", "Ines", "Noor", "Kai", "Elif", "Tomás"];
    let last = ["Lee", "Berg", "Núñez", "O'Brien", "Smith", "Kowalski", "Haddad", "Yamamoto", "Okafor", "Lindqvist"];

    let mut s = String::from("org_id,org_name,industry,website,annual_revenue,tier,created_on\n");
    for i in 1..=ORGS {
        let _ = writeln!(
            s,
            "{i},\"{} {} {}\",{},https://org{i}.example,{}.{:02},{},20{:02}-{:02}-{:02}",
            r.pick(&["Northwind", "Contoso", "Fabrikam", "Globex", "Initech", "Umbrella", "Hooli", "Vandelay"]),
            r.pick(&["Trading", "Systems", "Labs", "Group", "Industries", "Partners"]),
            r.pick(&["AB", "Ltd", "GmbH", "Inc", "Oy"]),
            r.pick(&industries),
            r.below(9_000_000) + 10_000,
            r.below(100),
            r.pick(&["gold", "silver", "bronze", ""]),
            10 + r.below(15),
            1 + r.below(12),
            1 + r.below(28)
        );
    }
    std::fs::write(dir.join("organizations.csv"), &s)?;

    s = String::from("contact_id,org_id,first_name,last_name,email,phone,linkedin\n");
    for i in 1..=CONTACTS {
        let email = if r.below(12) == 0 { String::new() } else { format!("person{i}@org.example") };
        let linkedin = if r.below(2) == 0 { format!("https://linkedin.example/in/p{i}") } else { String::new() };
        let _ = writeln!(
            s,
            "C{i:06},{},{},\"{}\",{email},+46 70 {:03} {:02} {:02},{linkedin}",
            1 + r.below(ORGS as u64),
            r.pick(&first),
            r.pick(&last),
            r.below(1000),
            r.below(100),
            r.below(100)
        );
    }
    std::fs::write(dir.join("contacts.csv"), &s)?;

    s = String::from("opp_id,org_id,contact_id,title,amount,currency,stage,close_date\n");
    for i in 1..=OPPORTUNITIES {
        let _ = writeln!(
            s,
            "{i},{},C{:06},\"{} {}\",{}.{:02},{},{},2026-{:02}-{:02}",
            1 + r.below(ORGS as u64),
            1 + r.below(CONTACTS as u64),
            r.pick(&["Renewal", "Expansion", "Pilot", "Rollout", "Upgrade"]),
            r.pick(&["Q1", "Q2", "Q3", "Q4"]),
            r.below(500_000),
            r.below(100),
            r.pick(&["SEK", "EUR", "USD"]),
            r.pick(&["prospect", "negotiation", "won", "lost"]),
            1 + r.below(12),
            1 + r.below(28)
        );
    }
    std::fs::write(dir.join("opportunities.csv"), &s)?;

    s = String::from("act_id,contact_id,kind,subject,occurred_at\n");
    for i in 1..=ACTIVITIES {
        let _ = writeln!(
            s,
            "A{i:06},C{:06},{},\"{} #{i}\",2025-{:02}-{:02}T{:02}:{:02}:00Z",
            1 + r.below(CONTACTS as u64),
            r.pick(&["call", "email", "meeting", "note"]),
            r.pick(&["Follow-up", "Intro", "Contract review", "Support"]),
            1 + r.below(12),
            1 + r.below(28),
            r.below(24),
            r.below(60)
        );
    }
    std::fs::write(dir.join("activities.csv"), &s)?;
    Ok(())
}

/// Create (or refresh) the demo connections. The destination is a new, empty folder each time,
/// so the demo can be repeated.
pub async fn setup(app: &App) -> MaResult<Value> {
    let root = app.home().join("demo");
    let source = root.join("legacy-crm-export");
    if !source.join("activities.csv").exists() {
        write_source(&source)?;
    }
    let output = root.join(format!("new-system-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S")));
    std::fs::create_dir_all(&output)?;
    let conn = |name: &str, path: &Path, settings: Value| ConnectionConfig {
        name: name.into(),
        kind: "files".into(),
        environment: Environment::Test,
        settings: {
            let mut s = settings;
            s["path"] = json!(path.display().to_string());
            s
        },
        credential_ref: None,
        allowed_hosts: vec![],
        connector_ref: None,
    };
    app.add_connection(
        conn(
            SOURCE,
            &source,
            json!({"format": "csv", "id_fields": {"organizations": "org_id", "contacts": "contact_id", "opportunities": "opp_id", "activities": "act_id"}}),
        ),
        Actor::Ui,
    )
    .await?;
    app.add_connection(conn(DESTINATION, &output, json!({"format": "jsonl"})), Actor::Ui).await?;
    let total = ORGS + CONTACTS + OPPORTUNITIES + ACTIVITIES;
    Ok(json!({
        "source": SOURCE, "destination": DESTINATION, "records": total,
        "source_path": source.display().to_string(), "output_path": output.display().to_string(),
    }))
}

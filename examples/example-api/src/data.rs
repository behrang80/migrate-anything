//! Deterministic seed data for the FieldOps Legacy Service.

use serde_json::{Value, json};

/// SplitMix64: tiny, fast, deterministic PRNG (no external crates).
pub(crate) struct SplitMix64(u64);

impl SplitMix64 {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform-ish integer in `0..n` (n > 0).
    pub(crate) fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    /// Inclusive range.
    pub(crate) fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi - lo + 1)
    }

    /// True with probability `pct`/100.
    pub(crate) fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }

    pub(crate) fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

const FIRST_NAMES: &[&str] = &[
    "Zoë", "Søren", "Ana", "Marcus", "Priya", "José", "Ingrid", "Kwame", "Mei", "Björn", "Fatima", "Liam", "Chloé", "Dmitri", "Aiyana",
    "Noah", "Renée", "Tomás", "Yuki", "Olu",
];

const LAST_NAMES: &[&str] = &[
    "Andersen",
    "Okafor",
    "Müller",
    "García",
    "Nakamura",
    "Kowalski",
    "O'Brien",
    "Haddad",
    "Silva",
    "Lindqvist",
    "Patel",
    "Dubois",
    "Novák",
    "Fernández",
    "Chen",
    "Mbeki",
    "Øster",
    "Rossi",
];

const REGIONS: &[&str] = &["NW-01", "NW-02", "SE-03", "SE-04", "CE-05", "MT-06", "PC-07"];

const BADGE_COLORS: &[&str] = &["red", "blue", "green", "orange", "purple", "silver"];

const TITLE_VERBS: &[&str] = &["Repair", "Inspect", "Replace", "Install", "Service", "Calibrate", "Diagnose"];

const TITLE_OBJECTS: &[&str] = &[
    "HVAC unit",
    "boiler",
    "rooftop condenser",
    "backflow preventer",
    "fire alarm panel",
    "walk-in freezer",
    "generator",
    "elevator controller",
    "water heater",
    "solar inverter",
];

const STREETS: &[&str] =
    &["Main St", "Oak Avenue", "Industrial Pkwy", "Harbor Rd", "Königstraße", "Rue de la Paix", "Elm Court", "Calle Mayor"];

const CITIES: &[(&str, &str)] =
    &[("Portland", "97"), ("Malmö", "21"), ("Zürich", "80"), ("Austin", "73"), ("Montréal", "H2"), ("Denver", "80"), ("São Paulo", "01")];

const TAGS: &[&str] = &["warranty", "after-hours", "parts-needed", "repeat-visit", "safety", "vip"];

const STATUSES: &[&str] = &["open", "scheduled", "done", "cancelled"];

fn ascii_fold(s: &str) -> String {
    s.chars()
        .filter_map(|c| {
            let mapped = match c {
                'ë' | 'é' | 'è' => 'e',
                'ø' | 'Ø' | 'ö' => 'o',
                'ü' => 'u',
                'á' | 'ã' => 'a',
                'í' => 'i',
                'ß' => 's',
                c if c.is_ascii_alphanumeric() => c.to_ascii_lowercase(),
                _ => return None,
            };
            Some(mapped)
        })
        .collect()
}

pub(crate) fn technician_id(n: usize) -> String {
    format!("T-{n:04}")
}

pub(crate) fn generate_technicians(rng: &mut SplitMix64, count: usize) -> Vec<Value> {
    (1..=count)
        .map(|n| {
            let first = *rng.pick(FIRST_NAMES);
            let last = *rng.pick(LAST_NAMES);
            let email = if rng.chance(15) {
                Value::Null
            } else {
                Value::String(format!("{}.{}{}@fieldops.example", ascii_fold(first), ascii_fold(last), n))
            };
            let skill_roll = rng.below(100);
            let skill = match skill_roll {
                0..=49 => "junior",
                50..=84 => "senior",
                _ => "lead",
            };
            let quarter = [0.0, 0.25, 0.5, 0.75][rng.below(4) as usize];
            let hourly_rate = rng.range(35, 90) as f64 + quarter;
            let badge = if rng.chance(30) { Value::String((*rng.pick(BADGE_COLORS)).to_owned()) } else { Value::Null };
            json!({
                "tech_id": technician_id(n),
                "full_name": format!("{first} {last}"),
                "email": email,
                "skill_level": skill,
                "hourly_rate": hourly_rate,
                "active": rng.chance(85),
                "hired_on": format!(
                    "{:04}-{:02}-{:02}",
                    rng.range(2010, 2024),
                    rng.range(1, 12),
                    rng.range(1, 28)
                ),
                "region_code": *rng.pick(REGIONS),
                "x_badge_color": badge,
            })
        })
        .collect()
}

/// First server-assigned work order number when the store is empty.
pub(crate) const FIRST_WO_NUMBER: i64 = 100_001;

pub(crate) fn generate_work_orders(rng: &mut SplitMix64, count: usize, technicians: usize) -> Vec<Value> {
    let mut wo_number = FIRST_WO_NUMBER;
    (0..count)
        .map(|_| {
            let title = format!("{} {}", rng.pick(TITLE_VERBS), rng.pick(TITLE_OBJECTS));
            let status = *rng.pick(STATUSES);
            let assigned = if technicians == 0 || rng.chance(10) {
                Value::Null
            } else {
                Value::String(technician_id(rng.range(1, technicians as u64) as usize))
            };
            let (city, zip_prefix) = *rng.pick(CITIES);
            let mut tags = Vec::new();
            for _ in 0..rng.below(4) {
                let tag = *rng.pick(TAGS);
                if !tags.contains(&tag) {
                    tags.push(tag);
                }
            }
            let wo = json!({
                "wo_number": wo_number,
                "title": title,
                "status": status,
                "assigned_tech": assigned,
                "priority": rng.range(1, 5),
                "estimated_hours": rng.range(2, 64) as f64 * 0.25,
                "site": {
                    "street": format!("{} {}", rng.range(1, 9999), rng.pick(STREETS)),
                    "city": city,
                    "postal_code": format!("{zip_prefix}{:03}", rng.below(1000)),
                },
                "opened_at": format!(
                    "2024-{:02}-{:02}T{:02}:{:02}:{:02}Z",
                    rng.range(1, 12),
                    rng.range(1, 28),
                    rng.range(6, 19),
                    rng.below(60),
                    rng.below(60)
                ),
                "tags": tags,
            });
            // Occasional gaps in numbering, like a real legacy system.
            wo_number += if rng.chance(10) { rng.range(2, 5) as i64 } else { 1 };
            wo
        })
        .collect()
}

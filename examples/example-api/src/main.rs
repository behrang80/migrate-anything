//! `ma-example-api [--port 8088] [--seed 42] [--api-key KEY] [--technicians N]
//! [--work-orders N] [--rate-limit-every N] [--empty]`
//!
//! Serves the FieldOps Legacy Service until Ctrl-C.

use ma_example_api::{ApiConfig, start_on};

const USAGE: &str = "usage: ma-example-api [--port 8088] [--seed 42] [--api-key KEY] \
[--technicians N] [--work-orders N] [--rate-limit-every N] [--empty]";

fn parse_args() -> Result<(ApiConfig, u16), String> {
    let mut config = ApiConfig::default();
    let mut port: u16 = 8088;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("missing value for {name}"));
        fn num<T: std::str::FromStr>(name: &str, raw: String) -> Result<T, String> {
            raw.parse().map_err(|_| format!("invalid value for {name}: {raw}"))
        }
        match arg.as_str() {
            "--port" => port = num("--port", value("--port")?)?,
            "--seed" => config.seed = num("--seed", value("--seed")?)?,
            "--api-key" => config.api_key = value("--api-key")?,
            "--technicians" => config.technicians = num("--technicians", value("--technicians")?)?,
            "--work-orders" => config.work_orders = num("--work-orders", value("--work-orders")?)?,
            "--rate-limit-every" => {
                config.rate_limit_every = Some(num("--rate-limit-every", value("--rate-limit-every")?)?);
            }
            "--empty" => config.empty = true,
            "-h" | "--help" => return Err(USAGE.to_owned()),
            other => return Err(format!("unknown argument: {other}\n{USAGE}")),
        }
    }
    Ok((config, port))
}

#[tokio::main]
async fn main() {
    let (config, port) = match parse_args() {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };
    let api = match start_on(config, port).await {
        Ok(api) => api,
        Err(e) => {
            eprintln!("failed to start server on port {port}: {e}");
            std::process::exit(1);
        }
    };
    println!("FieldOps Legacy Service listening on {}", api.base_url);
    println!("OpenAPI document: {}/openapi.json", api.base_url);
    if let Err(e) = tokio::signal::ctrl_c().await {
        eprintln!("failed to listen for Ctrl-C: {e}");
    }
    println!("shutting down");
    api.shutdown();
}

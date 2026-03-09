use clap::Parser;
use rtt_core::benchmark::{self, BenchmarkConfig, BenchmarkMode};
use rtt_core::connection::AddressFamily;
use rtt_core::polymarket::{CLOB_HOST, CLOB_PORT};

#[derive(Parser, Debug)]
#[command(name = "rtt-bench", about = "RTT benchmark harness")]
struct Cli {
    /// Run a single trigger test
    #[arg(long)]
    trigger_test: bool,

    /// Run benchmark
    #[arg(long)]
    benchmark: bool,

    /// Target host
    #[arg(long, default_value_t = CLOB_HOST.to_string())]
    host: String,

    /// Target port
    #[arg(long, default_value_t = CLOB_PORT)]
    port: u16,

    /// Benchmark mode: single-shot, random-cadence, burst-race
    #[arg(long, default_value = "single-shot")]
    mode: String,

    /// Number of trigger samples
    #[arg(long, default_value = "100")]
    samples: usize,

    /// Connection pool size
    #[arg(long, default_value = "2")]
    connections: usize,

    /// Triggers per burst (burst-race mode)
    #[arg(long, default_value = "5")]
    burst_size: usize,

    /// Minimum interval in ms (random-cadence mode)
    #[arg(long, default_value = "50")]
    min_interval: u32,

    /// Maximum interval in ms (random-cadence mode)
    #[arg(long, default_value = "500")]
    max_interval: u32,

    /// CPU core to pin execution thread (-1 = no pin)
    #[arg(long, default_value = "-1")]
    pin_core: i32,

    /// Address family: auto, v4, v6 (default v6: tighter tail latency)
    #[arg(long, default_value = "v6")]
    af: String,
}

fn parse_mode(s: &str) -> BenchmarkMode {
    match s {
        "single-shot" => BenchmarkMode::SingleShot,
        "random-cadence" => BenchmarkMode::RandomCadence,
        "burst-race" => BenchmarkMode::BurstRace,
        _ => {
            eprintln!("Unknown mode '{}', defaulting to single-shot", s);
            BenchmarkMode::SingleShot
        }
    }
}

fn parse_af(s: &str) -> AddressFamily {
    match s {
        "v4" => AddressFamily::V4,
        "v6" => AddressFamily::V6,
        "auto" | _ => AddressFamily::Auto,
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if cli.trigger_test {
        eprintln!(
            "Running single trigger test against {}:{}...",
            cli.host, cli.port
        );
        let config = BenchmarkConfig {
            host: cli.host,
            port: cli.port,
            mode: BenchmarkMode::SingleShot,
            sample_count: 1,
            pool_size: 1,
            address_family: parse_af(&cli.af),
            ..Default::default()
        };
        match benchmark::run_benchmark(&config).await {
            Ok(result) => {
                benchmark::print_report(&result);
            }
            Err(e) => {
                eprintln!("Trigger test failed: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    if cli.benchmark {
        let config = BenchmarkConfig {
            host: cli.host,
            port: cli.port,
            mode: parse_mode(&cli.mode),
            sample_count: cli.samples,
            pool_size: cli.connections,
            burst_size: cli.burst_size,
            min_interval_ms: cli.min_interval,
            max_interval_ms: cli.max_interval,
            pin_core: if cli.pin_core >= 0 {
                Some(cli.pin_core as usize)
            } else {
                None
            },
            address_family: parse_af(&cli.af),
        };

        eprintln!(
            "Running {:?} benchmark: {} samples, {} connections, host={}:{}, af={:?}",
            config.mode,
            config.sample_count,
            config.pool_size,
            config.host,
            config.port,
            config.address_family
        );

        match benchmark::run_benchmark(&config).await {
            Ok(result) => {
                benchmark::print_report(&result);
            }
            Err(e) => {
                eprintln!("Benchmark failed: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    eprintln!("Usage: rtt-bench --trigger-test | --benchmark [options]");
    eprintln!("Run with --help for full options.");
}

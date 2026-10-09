//! Command-line interface.

mod run;

use std::path::{Path, PathBuf};
use std::process::{Command as Process, ExitCode};
use std::{env, fs};

use anyhow::{Context, bail};
use chrono::{Local, Utc};
use clap::{Args, Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::config::{self, ConfigSource};
use crate::providers::{self, FsRoot};
use crate::reprice::{self, Totals};
use crate::store::Store;
use crate::tariff_form::{self, TariffForm};
use crate::time_range::{self, Range, Span};
use crate::{report, series, setup};

#[derive(Parser)]
#[command(
    version,
    about = "Measure your computer's power draw and what it costs"
)]
struct Cli {
    /// Config file (default: ~/.config/wattcost/config.toml)
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show which sensors are available and the config in use
    Detect,
    /// Collect measurements in the foreground
    Run,
    /// Summarise energy and cost (default: today)
    Report(ReportArgs),
    /// Recompute stored prices for a date range with the current config
    Reprice(RepriceArgs),
    /// Show or change the tariff that the panel settings form edits
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Print cost and power of a span as JSON, for the panel indicator
    Series {
        #[arg(long, value_enum, default_value = "day")]
        span: Span,
    },
    /// Install the systemd user service and explain how to enable CPU energy readings
    Setup {
        /// Write the unit file but do not enable or start it
        #[arg(long)]
        no_enable: bool,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the currency and the day and night periods with final prices as JSON
    Show,
    /// Read that JSON from stdin, check it and save it to the config file
    Set,
}

#[derive(Args)]
struct ReportArgs {
    #[arg(long, conflicts_with_all = ["week", "month", "from"])]
    day: bool,
    #[arg(long, conflicts_with_all = ["month", "from"])]
    week: bool,
    #[arg(long, conflicts_with = "from")]
    month: bool,
    /// First local date, YYYY-MM-DD
    #[arg(long)]
    from: Option<String>,
    /// Last local date (inclusive), YYYY-MM-DD; default: today
    #[arg(long, requires = "from")]
    to: Option<String>,
}

#[derive(Args)]
struct RepriceArgs {
    /// First local date, YYYY-MM-DD
    #[arg(long)]
    from: String,
    /// Last local date (inclusive), YYYY-MM-DD; default: today
    #[arg(long)]
    to: Option<String>,
    /// Show the totals without writing
    #[arg(long)]
    dry_run: bool,
}

pub fn main() -> ExitCode {
    init_logging();
    let cli = Cli::parse();
    let config_path = cli.config.unwrap_or_else(config::default_path);
    let result = match cli.command {
        Command::Detect => detect(&config_path),
        Command::Run => run::run(&config_path),
        Command::Report(args) => report(&config_path, &args),
        Command::Reprice(args) => reprice(&config_path, &args),
        Command::Series { span } => print_series(&config_path, span),
        Command::Config {
            action: ConfigAction::Show,
        } => show_tariff(&config_path),
        Command::Config {
            action: ConfigAction::Set,
        } => set_tariff(&config_path),
        Command::Setup { no_enable } => install(&config_path, no_enable),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    // journald adds its own timestamps.
    if env::var_os("JOURNAL_STREAM").is_some() {
        builder.without_time().init();
    } else {
        builder.init();
    }
}

fn detect(config_path: &Path) -> anyhow::Result<()> {
    let (settings, source) = config::load_or_example(config_path)?;
    let detected = providers::detect(&FsRoot::default(), &settings.config.hardware);
    println!("Providers");
    for probe in &detected.probes {
        for (quality, detail) in probe.summary() {
            println!("  {:<13} {quality:<10} {detail}", probe.provider);
        }
    }
    match source {
        ConfigSource::File(path) => println!("Config    {}", path.display()),
        ConfigSource::Example { missing } => {
            println!(
                "Config    example values (create {} to use your own)",
                missing.display()
            );
        }
    }
    println!(
        "Database  {}",
        settings.config.collector.database_path().display()
    );
    Ok(())
}

fn open_existing_store(settings: &config::Settings) -> anyhow::Result<Option<Store>> {
    let database = settings.config.collector.database_path();
    if !database.exists() {
        return Ok(None);
    }
    let store =
        Store::open(&database).with_context(|| format!("opening {}", database.display()))?;
    Ok(Some(store))
}

fn report(config_path: &Path, args: &ReportArgs) -> anyhow::Result<()> {
    let today = Local::now().date_naive();
    let range: Range = match &args.from {
        Some(from) => time_range::dates_range(from, args.to.as_deref(), today, &Local)?,
        None => {
            let span = if args.week {
                Span::Week
            } else if args.month {
                Span::Month
            } else {
                Span::Day
            };
            time_range::span_range(span, today, &Local)?
        }
    };
    let (settings, _) = config::load_or_example(config_path)?;
    let rows = match open_existing_store(&settings)? {
        Some(store) => store.samples(range.from, range.to)?,
        None => Vec::new(),
    };
    print!(
        "{}",
        report::summarize(range.label, &rows, &settings.config.hardware)
    );
    Ok(())
}

fn show_tariff(config_path: &Path) -> anyhow::Result<()> {
    let (settings, _) = config::load_or_example(config_path)?;
    println!(
        "{}",
        serde_json::to_string(&TariffForm::from_settings(&settings)?)?
    );
    Ok(())
}

fn set_tariff(config_path: &Path) -> anyhow::Result<()> {
    let form: TariffForm =
        serde_json::from_reader(std::io::stdin().lock()).context("reading the tariff JSON")?;
    let settings = form.apply(&tariff_form::current_text(config_path)?)?;
    tariff_form::write_atomically(config_path, &settings.text)?;
    Ok(())
}

fn print_series(config_path: &Path, span: Span) -> anyhow::Result<()> {
    let (settings, _) = config::load_or_example(config_path)?;
    let today = Local::now().date_naive();
    let range = time_range::span_range(span, today, &Local)?;
    let rows = match open_existing_store(&settings)? {
        Some(store) => store.samples(range.from, range.to)?,
        None => Vec::new(),
    };
    let series = series::build(
        span,
        today,
        &Local,
        &rows,
        settings.tariff.currency(),
        &settings.config.hardware,
    )?;
    println!("{}", serde_json::to_string(&series)?);
    Ok(())
}

fn reprice(config_path: &Path, args: &RepriceArgs) -> anyhow::Result<()> {
    let (settings, _) = config::load_or_example(config_path)?;
    let Some(mut store) = open_existing_store(&settings)? else {
        bail!("no database yet; start `wattcost run` first");
    };
    let range = time_range::dates_range(
        &args.from,
        args.to.as_deref(),
        Local::now().date_naive(),
        &Local,
    )?;
    let outcome = reprice::reprice(
        &mut store,
        &settings,
        &Local,
        &range,
        args.dry_run,
        Utc::now(),
    )?;
    println!("{}: {} rows", range.label, outcome.rows);
    println!(
        "Energy  {:.3} kWh -> {:.3} kWh",
        outcome.before.wall_wh / 1000.0,
        outcome.after.wall_wh / 1000.0
    );
    print_costs(&outcome.before, &outcome.after);
    println!(
        "{}",
        if outcome.applied {
            "Updated."
        } else {
            "Nothing written."
        }
    );
    Ok(())
}

fn print_costs(before: &Totals, after: &Totals) {
    let currencies: std::collections::BTreeSet<&String> =
        before.cost.keys().chain(after.cost.keys()).collect();
    for currency in currencies {
        let old = before.cost.get(currency).copied().unwrap_or(0.0);
        let new = after.cost.get(currency).copied().unwrap_or(0.0);
        println!("Cost    {old:.2} {currency} -> {new:.2} {currency}");
    }
}

fn install(config_path: &Path, no_enable: bool) -> anyhow::Result<()> {
    let exe = env::current_exe().context("locating the wattcost binary")?;
    let unit_path = setup::unit_path();
    if let Some(dir) = unit_path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    // The service starts in the home directory, so a relative path would point elsewhere.
    let config_path = std::path::absolute(config_path).context("resolving the config path")?;
    let unit = setup::unit_file(&exe, &config_path).map_err(anyhow::Error::msg)?;
    fs::write(&unit_path, unit).with_context(|| format!("writing {}", unit_path.display()))?;
    println!("Wrote {}", unit_path.display());
    if !no_enable {
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "--now", setup::UNIT_NAME])?;
        println!("Enabled and started {}", setup::UNIT_NAME);
        if !linger_enabled() {
            println!("{}", setup::LINGER_HINT);
        }
    }
    let (settings, _) = config::load_or_example(&config_path)?;
    let detected = providers::detect(&FsRoot::default(), &settings.config.hardware);
    if detected.found("rapl") {
        println!("CPU energy (RAPL) is readable; nothing else to do.");
    } else {
        println!("\n{}", setup::UDEV_INSTRUCTIONS);
    }
    Ok(())
}

/// Whether systemd keeps this user's services running without a login session.
fn linger_enabled() -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(uid) = fs::metadata("/proc/self").map(|meta| meta.uid()) else {
        return false;
    };
    Process::new("loginctl")
        .args([
            "show-user",
            &uid.to_string(),
            "--property=Linger",
            "--value",
        ])
        .output()
        .is_ok_and(|output| output.stdout.trim_ascii() == b"yes")
}

fn systemctl(args: &[&str]) -> anyhow::Result<()> {
    let status = Process::new("systemctl")
        .arg("--user")
        .args(args)
        .status()
        .context("running systemctl")?;
    if !status.success() {
        bail!("systemctl --user {} failed with {status}", args.join(" "));
    }
    Ok(())
}

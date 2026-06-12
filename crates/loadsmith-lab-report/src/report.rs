use std::path::PathBuf;
use std::time::Duration;

use colored::Colorize;

#[derive(Debug, Default)]
pub struct CaseResult {
    pub name: String,
    pub description: Option<String>,
    pub passed: bool,
    pub duration: Duration,
    pub rows_read: Option<u64>,
    pub rows_written: Option<u64>,
    /// Setup steps (image builds, service readiness) shown before the run.
    pub prep_lines: Vec<String>,
    /// loadsmith's own stdout report, shown framed under the case.
    pub loadsmith_output: String,
    pub failures: Vec<String>,
    /// Host path of the per-run output directory (mounted at /output during the
    /// run). Bundles need this to hand the produced files to validate/cleanup
    /// hooks; the plain `run` path ignores it.
    pub output_dir: PathBuf,
}

/// Prints a single prep/setup line immediately (image resolution, readiness).
/// Called inline during case execution so the user sees it in real time.
pub fn print_prep_line(line: &str) {
    println!("  {} {}", "·".dimmed(), line.dimmed());
}

/// Prints a single line of loadsmith output inside the │ gutter, in real time.
pub fn print_gutter_line(line: &str) {
    println!("  {} {line}", "│".dimmed());
}

/// Top banner printed once at the start of a run.
pub fn print_banner(lab_version: &str, mode: &str) {
    println!();
    println!(
        "{} {}  {}  {}",
        "loadsmith-lab".bold(),
        lab_version.dimmed(),
        "·".dimmed(),
        mode.dimmed()
    );
}

/// Header printed when a case starts.
pub fn print_case_header(name: &str, description: Option<&str>) {
    println!();
    match description {
        Some(d) => println!("{} {}  {}", "▶".cyan().bold(), name.bold(), d.dimmed()),
        None => println!("{} {}", "▶".cyan().bold(), name.bold()),
    }
}

/// Prints a case result: prep lines, the framed loadsmith report, and a verdict.
pub fn print_result(result: &CaseResult) {
    for line in &result.prep_lines {
        println!("  {} {}", "·".dimmed(), line.dimmed());
    }

    // Frame loadsmith's own output with a dim gutter.
    let trimmed = result.loadsmith_output.trim_matches('\n');
    if !trimmed.is_empty() {
        let gutter = "│".dimmed();
        for line in trimmed.lines() {
            println!("  {gutter} {line}");
        }
    }

    let dur = format!("{:.1}s", result.duration.as_secs_f64());
    let rows = match (result.rows_read, result.rows_written) {
        (Some(r), Some(w)) => format!("{} read · {} written", fmt_n(r), fmt_n(w)),
        (Some(r), None) => format!("{} read", fmt_n(r)),
        (None, Some(w)) => format!("{} written", fmt_n(w)),
        (None, None) => String::new(),
    };

    if result.passed {
        let icon = "✓".green().bold();
        if rows.is_empty() {
            println!("  {} {}   {}", icon, result.name.green(), dur.dimmed());
        } else {
            println!("  {} {}   {}   {}", icon, result.name.green(), rows, dur.dimmed());
        }
    } else {
        println!("  {} {}   {}", "✗".red().bold(), result.name.red(), dur.dimmed());
        for f in &result.failures {
            println!("    {} {}", "→".red(), f.red());
        }
    }
}

pub fn print_summary(results: &[CaseResult]) {
    let passed = results.iter().filter(|r| r.passed).count();
    let failed = results.len() - passed;
    println!();
    println!("{}", "─".repeat(50).dimmed());
    let summary = format!("{passed} passed, {failed} failed");
    if failed > 0 {
        println!("{}", summary.red().bold());
    } else {
        println!("{}", summary.green().bold());
    }
}

fn fmt_n(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

// ── Bundles ────────────────────────────────────────────────────────────────

/// Outcome of one bundle entry: the wrapped case plus its setup/validate/cleanup
/// hook results. An entry passes only if setup (if any) succeeded, the case
/// passed, and validate (if any) succeeded; a cleanup failure is a warning only.
#[derive(Debug, Default)]
pub struct BundleEntryResult {
    pub case_name: String,
    pub passed: bool,
    /// The case run itself — `None` only when setup failed before the case ran.
    pub case_result: Option<CaseResult>,
    pub setup_failure: Option<String>,
    pub validate_failure: Option<String>,
    pub cleanup_warning: Option<String>,
}

#[derive(Debug, Default)]
pub struct BundleResult {
    pub name: String,
    pub entries: Vec<BundleEntryResult>,
}

/// Header printed once when a bundle starts.
pub fn print_bundle_header(name: &str, description: Option<&str>) {
    println!();
    match description {
        Some(d) => println!("{} {}  {}", "■".magenta().bold(), name.bold(), d.dimmed()),
        None => println!("{} {}", "■".magenta().bold(), name.bold()),
    }
}

/// Header printed for each hook phase, so the user sees setup/validate/cleanup
/// boundaries in the live stream.
pub fn print_hook_line(line: &str) {
    println!("  {} {}", "·".dimmed(), line.dimmed());
}

/// Prints one bundle entry: the embedded case result (reusing `print_result`),
/// then the setup/validate verdicts and any cleanup warning.
pub fn print_entry_result(entry: &BundleEntryResult) {
    if let Some(setup) = &entry.setup_failure {
        println!("  {} setup: {}", "✗".red().bold(), setup.red());
    }
    if let Some(case) = &entry.case_result {
        print_result(case);
    }
    if let Some(v) = &entry.validate_failure {
        println!("  {} validate: {}", "✗".red().bold(), v.red());
    }
    if let Some(w) = &entry.cleanup_warning {
        println!("  {} cleanup: {}", "!".yellow().bold(), w.yellow());
    }
}

pub fn print_bundle_summary(result: &BundleResult) {
    let passed = result.entries.iter().filter(|e| e.passed).count();
    let failed = result.entries.len() - passed;
    println!();
    println!("{}", "─".repeat(50).dimmed());
    let summary = format!("{} — {passed} passed, {failed} failed", result.name);
    if failed > 0 {
        println!("{}", summary.red().bold());
    } else {
        println!("{}", summary.green().bold());
    }
}

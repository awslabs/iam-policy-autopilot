//! `iac-paper-figures` — reads `aggregate_report.json` produced by
//! `iac-benchmarker --runs-dir` and emits LaTeX artefacts for a paper's
//! experimental-evaluation section.
//!
//! Outputs (written to --output-dir, default: `./paper_figures/`):
//!   overpermission_boxplot.tex   — pgfplots strip/box plot figure (log10 scale)
//!   stats_table.tex              — booktabs summary-statistics table (single-column)
//!                                    Columns: Policy, Mean, Med., Std, Min, Max
//!   raw_data.tex                 — per-run validation results (table*, single tabular)
//!                                    Cells: checkmark/times only; Pass summary row
//!
//! Required LaTeX packages (add to your preamble):
//!   \usepackage{pgfplots}
//!   \usepgfplotslibrary{statistics}
//!   \pgfplotsset{compat=1.18}
//!   \usepackage{booktabs}
//!   \usepackage{siunitx}
//!   % NOTE: do NOT use longtable with IEEEtran two-column mode.
//!
//! Usage:
//!   iac-paper-figures --input aggregate_report.json
//!   iac-paper-figures --input aggregate_report.json --output-dir paper/figures/

use std::{
    collections::BTreeMap,
    fmt::Write as FmtWrite,
    fs,
    path::PathBuf,
};

use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;
use iac_benchmarker::aggregate::{AggregateReport, Stats};

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Debug, Parser)]
#[command(
    name = "iac-paper-figures",
    about = "Generate LaTeX figures and tables from an aggregate_report.json"
)]
struct Cli {
    /// Path to the aggregate_report.json produced by iac-benchmarker --runs-dir.
    #[arg(long, short = 'i', default_value = "aggregate_report.json")]
    input: PathBuf,

    /// Path to the coverage_report.json produced by iac-coverage-analyzer (optional).
    #[arg(long, short = 'c')]
    coverage_report: Option<PathBuf>,

    /// Directory to write LaTeX output files into.
    #[arg(long, short = 'o', default_value = "paper_figures")]
    output_dir: PathBuf,

    /// Label prefix used in LaTeX \label{} commands (e.g. "sec:eval").
    #[arg(long, default_value = "fig")]
    label_prefix: String,

    /// Exclude specific experiment tags from all generated figures and tables
    /// (comma-separated, e.g. "CTX-LLM/bare,CTX-LLM/wildcards,CTX-LLM/resource-star").
    #[arg(long)]
    exclude_experiments: Option<String>,
}

// ---------------------------------------------------------------------------
// Coverage report types (deserialized from coverage_report.json)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CoverageReport {
    #[allow(dead_code)]
    total_runs: usize,
    aggregates: Vec<ApproachCoverageAggregate>,
}

#[derive(Debug, Deserialize)]
struct ApproachCoverageAggregate {
    approach: String,
    languages: Vec<LanguageCoverageAggregate>,
}

#[derive(Debug, Deserialize)]
struct LanguageCoverageAggregate {
    language: String,
    coverage: Stats,
    precision: Stats,
    f1: Stats,
    #[allow(dead_code)]
    excess_actions: Stats,
    #[allow(dead_code)]
    missing_actions: Stats,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let cli = Cli::parse();

    let text = fs::read_to_string(&cli.input)
        .with_context(|| format!("Cannot read {:?}", cli.input))?;
    let mut report: AggregateReport = serde_json::from_str(&text)
        .with_context(|| format!("Cannot parse {:?}", cli.input))?;

    // Strip excluded experiment tags when --exclude-experiments is set
    if let Some(ref exclude_str) = cli.exclude_experiments {
        let exclude_tags: Vec<String> = exclude_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        for tag in &exclude_tags {
            report.llm_experiment_aggregates.remove(tag);
            for run in &mut report.runs {
                run.llm_experiment_summaries.remove(tag);
            }
        }
        println!("[info] --exclude-experiments: excluded {} experiment tag(s) from output", exclude_tags.len());
    }

    fs::create_dir_all(&cli.output_dir)
        .with_context(|| format!("Cannot create output dir {:?}", cli.output_dir))?;

    // ── 0. Optionally load coverage report ───────────────────────────────────
    let cov_report: Option<CoverageReport> = if let Some(ref cov_path) = cli.coverage_report {
        let cov_text = fs::read_to_string(cov_path)
            .with_context(|| format!("Cannot read {:?}", cov_path))?;
        let cr: CoverageReport = serde_json::from_str(&cov_text)
            .with_context(|| format!("Cannot parse {:?}", cov_path))?;
        Some(cr)
    } else {
        None
    };

    // ── 1. Box-plot figures (detail + overview) ──────────────────────────────
    let boxplot = render_boxplot(&report, &cli.label_prefix, cov_report.as_ref());
    let boxplot_path = cli.output_dir.join("overpermission_boxplot.tex");
    fs::write(&boxplot_path, &boxplot)
        .with_context(|| format!("Cannot write {:?}", boxplot_path))?;
    println!("[saved] {:?}", boxplot_path);

    // ── 2. Summary statistics table ─────────────────────────────────────────
    let stats_table = render_stats_table(&report, &cli.label_prefix);
    let stats_path = cli.output_dir.join("stats_table.tex");
    fs::write(&stats_path, &stats_table)
        .with_context(|| format!("Cannot write {:?}", stats_path))?;
    println!("[saved] {:?}", stats_path);

    // ── 3. Per-run raw data table ────────────────────────────────────────────
    let raw_table = render_raw_data_table(&report, &cli.label_prefix);
    let raw_path = cli.output_dir.join("raw_data.tex");
    fs::write(&raw_path, &raw_table)
        .with_context(|| format!("Cannot write {:?}", raw_path))?;
    println!("[saved] {:?}", raw_path);

    // ── 4. Token usage table ─────────────────────────────────────────────────
    let token_table = render_token_usage_table(&report, &cli.label_prefix);
    if !token_table.is_empty() {
        let token_path = cli.output_dir.join("token_usage.tex");
        fs::write(&token_path, &token_table)
            .with_context(|| format!("Cannot write {:?}", token_path))?;
        println!("[saved] {:?}", token_path);
    }

    // ── 5. Token cost summary sentence ───────────────────────────────────────
    let cost_summary = render_token_cost_summary(&report);
    if !cost_summary.is_empty() {
        let cost_path = cli.output_dir.join("token_cost_summary.tex");
        fs::write(&cost_path, &cost_summary)
            .with_context(|| format!("Cannot write {:?}", cost_path))?;
        println!("[saved] {:?}", cost_path);
    }

    // ── 6. Coverage / precision / F1 table (from coverage_report.json) ───────
    if let Some(ref cr) = cov_report {
        let cov_table = render_coverage_table(cr, &cli.label_prefix);
        let cov_table_path = cli.output_dir.join("coverage_table.tex");
        fs::write(&cov_table_path, &cov_table)
            .with_context(|| format!("Cannot write {:?}", cov_table_path))?;
        println!("[saved] {:?}", cov_table_path);
    }

    println!();
    println!("Done. Include in your paper with:");
    println!("  \\input{{{}/overpermission_boxplot}}", cli.output_dir.display());
    println!("  \\input{{{}/stats_table}}", cli.output_dir.display());
    println!("  \\input{{{}/raw_data}}", cli.output_dir.display());
    println!("  \\input{{{}/token_usage}}", cli.output_dir.display());
    println!("  \\input{{{}/token_cost_summary}}", cli.output_dir.display());
    if cli.coverage_report.is_some() {
        println!("  \\input{{{}/coverage_table}}", cli.output_dir.display());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Figure 1: Horizontal boxplot — 9 aggregated groups
//
// rows (y=1 bottom to top):
//   IPA (Py)      — Autopilot, Python only
//   IPA (Go)      — Autopilot, Go only
//   IPA (Java)    — Autopilot, Java only
//   IPA (TS)      — Autopilot, TypeScript only
//   LLM_CTX       — All CTX-LLM/* strategies, all languages pooled
//   LLM           — All LLM/* strategies, all languages pooled
//   iamfast        — All iamfast languages pooled
//   Managed        — AWS managed policies
//
// Log10 x-axis.  Boxplot statistics computed in log10 space with 1.5×IQR
// whisker fences.  Mean diamond at log10(arithmetic_mean_in_linear_space).
// ---------------------------------------------------------------------------

fn render_boxplot(report: &AggregateReport, _label_prefix: &str, cov: Option<&CoverageReport>) -> String {
    let mut s = String::new();

    writeln!(s, "% Overpermissioning factor — horizontal box plot").unwrap();
    writeln!(s, "% Aggregated across languages and prompt strategies (except IPA, shown per-language)").unwrap();
    writeln!(s, "%").unwrap();
    writeln!(s, "% Required preamble:").unwrap();
    writeln!(s, "%   \\usepackage{{pgfplots}}").unwrap();
    writeln!(s, "%   \\usepgfplotslibrary{{statistics}}").unwrap();
    writeln!(s, "%   \\pgfplotsset{{compat=1.18}}").unwrap();
    writeln!(s, "%   \\usepackage{{xcolor}}").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "% Wong (2011) colour-blind-safe palette").unwrap();
    writeln!(s, "\\definecolor{{wongOrange}}{{RGB}}{{230,159,0}}").unwrap();
    writeln!(s, "\\definecolor{{wongSkyBlue}}{{RGB}}{{86,180,233}}").unwrap();
    writeln!(s, "\\definecolor{{wongBluishGreen}}{{RGB}}{{0,158,115}}").unwrap();
    writeln!(s, "\\definecolor{{wongBlue}}{{RGB}}{{0,114,178}}").unwrap();
    writeln!(s, "\\definecolor{{wongVermillion}}{{RGB}}{{213,94,0}}").unwrap();
    writeln!(s, "\\definecolor{{wongReddishPurple}}{{RGB}}{{204,121,167}}").unwrap();
    writeln!(s, "\\definecolor{{wongYellow}}{{RGB}}{{240,228,66}}").unwrap();
    writeln!(s, "\\definecolor{{wongBlack}}{{RGB}}{{0,0,0}}").unwrap();
    writeln!(s).unwrap();

    render_detail_figure(&mut s, report, cov);

    s
}

// ---------------------------------------------------------------------------
// Data structures for the horizontal boxplot
// ---------------------------------------------------------------------------

struct BoxplotGroup {
    y_pos: usize,
    label: String,
    /// LaTeX-formatted y-axis tick label for this group.
    ytick_label: String,
    /// Whether this group belongs to the IPA section (above the separator).
    is_ipa: bool,
    /// Coverage data key: "ipa:<lang>", "llm:<prefix>", "iamfast", or "managed".
    coverage_id: String,
    colour: &'static str,
    fill_opacity: f64,
    raw_factors: Vec<f64>,
}

struct BoxplotStats {
    lower_whisker: f64,
    q1: f64,
    median: f64,
    q3: f64,
    upper_whisker: f64,
    outliers: Vec<f64>,
    mean_log10: f64,
    arith_mean_linear: f64,
    n: usize,
}

/// Compute boxplot statistics in log10 space with 1.5×IQR whisker fences.
/// Returns `None` if no finite log10 values remain.
fn compute_boxplot_stats(values: &[f64]) -> Option<BoxplotStats> {
    let mut logs: Vec<f64> = values.iter()
        .map(|v| v.log10())
        .filter(|lv| lv.is_finite())
        .collect();
    if logs.is_empty() {
        return None;
    }
    logs.sort_by(|a, b| a.total_cmp(b));
    let n = logs.len();

    // R type 7 interpolation for quartiles
    let quantile = |p: f64| -> f64 {
        let h = p * (n as f64 - 1.0);
        let lo = h.floor() as usize;
        let hi = (lo + 1).min(n - 1);
        logs[lo] + (h - lo as f64) * (logs[hi] - logs[lo])
    };

    let q1 = quantile(0.25);
    let q3 = quantile(0.75);
    let iqr = q3 - q1;
    let fence_lo = q1 - 1.5 * iqr;
    let fence_hi = q3 + 1.5 * iqr;

    // Median: nearest-rank (always an actual data point)
    let median_idx = (n - 1) / 2;
    let median = logs[median_idx];

    // Whiskers: most extreme data points within the fences
    let lower_whisker = logs.iter().copied().find(|&v| v >= fence_lo).unwrap_or(logs[0]);
    let upper_whisker = logs.iter().rev().copied().find(|&v| v <= fence_hi).unwrap_or(logs[n - 1]);

    // Outliers: points outside the fences
    let outliers: Vec<f64> = logs.iter().copied()
        .filter(|&v| v < fence_lo || v > fence_hi)
        .collect();

    // Arithmetic mean in linear space, then log10
    let linear_vals: Vec<f64> = values.iter().copied().filter(|v| v.log10().is_finite()).collect();
    let arith_mean_linear = linear_vals.iter().sum::<f64>() / linear_vals.len() as f64;
    let mean_log10 = arith_mean_linear.log10();

    Some(BoxplotStats {
        lower_whisker,
        q1,
        median,
        q3,
        upper_whisker,
        outliers,
        mean_log10,
        arith_mean_linear,
        n,
    })
}

/// Format a linear factor as a tick label, e.g. `1.0` → `"$1{\times}$"`, `0.37` → `"$0.4{\times}$"`.
fn format_tick_label(linear: f64) -> String {
    if (linear - linear.round()).abs() < 1e-9 && linear >= 1.0 {
        format!("${}{{\\times}}$", linear.round() as i64)
    } else {
        // Use one decimal place for values < 10, no decimals for ≥ 10
        if linear >= 10.0 {
            format!("${}{{\\times}}$", linear.round() as i64)
        } else {
            format!("${:.1}{{\\times}}$", linear)
        }
    }
}

/// Compute dynamic x-axis ticks and bounds from the boxplot statistics.
///
/// Returns `(xmin, xmax, ticks)` where each tick is `(log10_value, label_string)`.
///
/// Fixed reference ticks at 1×, 2×, 5×, 10×, 50× are included when they fall
/// within the data range.  The actual data minimum and maximum (rounded to one
/// decimal in linear space) are always added as the outermost ticks.
fn compute_axis_ticks(stats: &[Option<BoxplotStats>]) -> (f64, f64, Vec<(f64, String)>) {
    // 1. Find global min/max in log10 space across whiskers + outliers
    let mut global_min = f64::INFINITY;
    let mut global_max = f64::NEG_INFINITY;
    for st in stats.iter().flatten() {
        global_min = global_min.min(st.lower_whisker);
        global_max = global_max.max(st.upper_whisker);
        for &ol in &st.outliers {
            global_min = global_min.min(ol);
            global_max = global_max.max(ol);
        }
    }

    // Fallback if no data
    if !global_min.is_finite() || !global_max.is_finite() {
        return (-0.5, 2.5, vec![
            (0.0, "$1{\\times}$".into()),
            (1.0, "$10{\\times}$".into()),
            (2.0, "$100{\\times}$".into()),
        ]);
    }

    // 2. Pad by 10% of range on each side (minimum 0.15 in log10 space)
    let range = global_max - global_min;
    let pad = (range * 0.10).max(0.15);
    let xmin = global_min - pad;
    let xmax = global_max + pad;

    // 3. Build tick list: fixed reference ticks + data min/max
    let fixed_refs: &[f64] = &[1.0, 2.0, 5.0, 10.0, 50.0];

    let mut ticks: Vec<(f64, String)> = Vec::new();

    // Add the data minimum as the first tick (round to 1 decimal in linear space)
    let data_min_linear = 10_f64.powf(global_min);
    let data_min_rounded = (data_min_linear * 10.0).round() / 10.0;
    let data_min_log = data_min_rounded.log10();
    if data_min_rounded > 0.0 && data_min_log.is_finite() {
        ticks.push((data_min_log, format_tick_label(data_min_rounded)));
    }

    // Add fixed reference ticks that fall within [global_min, global_max]
    for &factor in fixed_refs {
        let log_val = factor.log10();
        if log_val >= global_min && log_val <= global_max {
            // Skip if too close to the data-min tick we already added
            let dominated = ticks.iter().any(|(existing, _)| (existing - log_val).abs() < 0.08);
            if !dominated {
                ticks.push((log_val, format_tick_label(factor)));
            }
        }
    }

    // Add the data maximum as the last tick (round to nearest integer in linear space)
    let data_max_linear = 10_f64.powf(global_max);
    let data_max_rounded = if data_max_linear >= 10.0 {
        data_max_linear.round()
    } else {
        (data_max_linear * 10.0).round() / 10.0
    };
    let data_max_log = data_max_rounded.log10();
    if data_max_rounded > 0.0 && data_max_log.is_finite() {
        // Skip if too close to an existing tick
        let dominated = ticks.iter().any(|(existing, _)| (existing - data_max_log).abs() < 0.08);
        if !dominated {
            ticks.push((data_max_log, format_tick_label(data_max_rounded)));
        }
    }

    // Sort by log10 value
    ticks.sort_by(|a, b| a.0.total_cmp(&b.0));

    (xmin, xmax, ticks)
}

/// Format the mean annotation string, e.g. "$1.6{\times}$" or "$51{\times}$".
fn format_mean_annotation(mean_linear: f64) -> String {
    if mean_linear >= 10.0 {
        format!("${}{{\\times}}$", mean_linear.round() as i64)
    } else {
        format!("${:.1}{{\\times}}$", mean_linear)
    }
}

/// Collect all `over_permission_ratio_vs_minimal` values for a single IPA language
/// from the summary-level data in `report.languages`.
fn collect_ipa_factors(report: &AggregateReport, lang: &str) -> Vec<f64> {
    report.languages.iter()
        .filter(|la| la.language.eq_ignore_ascii_case(lang))
        .flat_map(|la| la.ratio_vs_minimal.values.clone())
        .collect()
}

/// Collect all `over_permission_ratio_vs_minimal` values for an LLM method group
/// (e.g. all keys starting with "CTX-LLM/"), pooled across languages and strategies.
fn collect_llm_factors(report: &AggregateReport, prefix: &str) -> Vec<f64> {
    report.llm_experiment_aggregates.iter()
        .filter(|(key, _)| key.starts_with(prefix))
        .flat_map(|(_, langs)| langs.iter().flat_map(|la| la.ratio_vs_minimal.values.clone()))
        .collect()
}

/// Collect all `over_permission_ratio_vs_minimal` values for iamfast,
/// pooled across all languages.
fn collect_iamfast_factors(report: &AggregateReport) -> Vec<f64> {
    report.iamfast_languages.iter()
        .flat_map(|la| la.ratio_vs_minimal.values.clone())
        .collect()
}

/// Check whether any experiment tags with the given prefix exist in the
/// aggregate report's `llm_experiment_aggregates`.
fn has_llm_prefix(report: &AggregateReport, prefix: &str) -> bool {
    report.llm_experiment_aggregates.keys().any(|k| k.starts_with(prefix))
}

/// Horizontal boxplot figure with dynamically built groups.
/// IPA shown per-language, LLM methods pooled across languages+strategies
/// (only groups with data are included), iamfast pooled, Managed.
///
/// When `cov` is provided, coverage/precision/F1 data is placed as TikZ `\node`
/// commands inside the axis environment using `axis cs` coordinates, so that
/// each table row is vertically aligned with its corresponding box-plot row.
/// The axis width is reduced to `0.82\columnwidth` to leave room for the nodes
/// that extend beyond `xmax` (enabled by `clip=false`).
fn render_detail_figure(s: &mut String, report: &AggregateReport, cov: Option<&CoverageReport>) {
    // Build candidate groups in top-to-bottom order (highest y first).
    // Groups with empty raw_factors are filtered out so that missing
    // experiments (e.g. CTX-LLM when no scenarios dir is configured) do not
    // produce empty rows.
    let mut candidates: Vec<BoxplotGroup> = Vec::new();

    // IPA per-language
    for &(lang, ytick) in &[
        ("python",     "IPA\\,(Python)"),
        ("go",         "IPA\\,(Go)"),
        ("java",       "IPA\\,(Java)"),
        ("typescript", "IPA\\,(TS)"),
    ] {
        let factors = collect_ipa_factors(report, lang);
        if !factors.is_empty() {
            candidates.push(BoxplotGroup {
                y_pos: 0, // assigned below
                label: format!("IPA ({})", short_lang_label(lang)),
                ytick_label: ytick.to_string(),
                is_ipa: true,
                coverage_id: format!("ipa:{}", lang),
                colour: "wongOrange",
                fill_opacity: if lang == "python" { 0.35 } else { 0.25 },
                raw_factors: factors,
            });
        }
    }

    // LLM groups — only include those with data in llm_experiment_aggregates
    let llm_group_defs: &[(&str, &str, &str, &str)] = &[
        ("CTX-LLM/", "LLM_{CTX}",     "$\\text{{LLM}}_{{\\text{{CTX}}}}$",          "wongBluishGreen"),
        ("LLM/",     "LLM",           "LLM",                                         "wongBlue"),
    ];
    for &(prefix, label, ytick, colour) in llm_group_defs {
        if has_llm_prefix(report, prefix) {
            let factors = collect_llm_factors(report, prefix);
            if !factors.is_empty() {
                candidates.push(BoxplotGroup {
                    y_pos: 0,
                    label: label.to_string(),
                    ytick_label: ytick.to_string(),
                    is_ipa: false,
                    coverage_id: format!("llm:{}", prefix),
                    colour,
                    fill_opacity: 0.25,
                    raw_factors: factors,
                });
            }
        }
    }

    // iamfast
    {
        let factors = collect_iamfast_factors(report);
        if !factors.is_empty() {
            candidates.push(BoxplotGroup {
                y_pos: 0,
                label: "iamfast".to_string(),
                ytick_label: "\\texttt{{iamfast}}".to_string(),
                is_ipa: false,
                coverage_id: "iamfast".to_string(),
                colour: "wongVermillion",
                fill_opacity: 0.25,
                raw_factors: factors,
            });
        }
    }

    // Managed
    {
        let factors = report.managed_vs_minimal_ratio.values.clone();
        if !factors.is_empty() {
            candidates.push(BoxplotGroup {
                y_pos: 0,
                label: "Managed".to_string(),
                ytick_label: "Managed".to_string(),
                is_ipa: false,
                coverage_id: "managed".to_string(),
                colour: "wongBlack",
                fill_opacity: 0.15,
                raw_factors: factors,
            });
        }
    }

    // Assign y-positions: y=N (top) down to y=1 (bottom)
    let n_groups = candidates.len();
    let groups: Vec<BoxplotGroup> = candidates.into_iter().enumerate().map(|(i, mut c)| {
        c.y_pos = n_groups - i;
        c
    }).collect();

    if groups.is_empty() {
        writeln!(s, "% No data available for boxplot").unwrap();
        return;
    }

    // Compute stats for each group
    let stats: Vec<Option<BoxplotStats>> = groups.iter()
        .map(|g| compute_boxplot_stats(&g.raw_factors))
        .collect();

    // Compute dynamic axis ticks and bounds from the data
    let (xmin, xmax, ticks) = compute_axis_ticks(&stats);
    let xtick_str: String = ticks.iter()
        .map(|(v, _)| format!("{:.4}", v))
        .collect::<Vec<_>>()
        .join(",");
    let xticklabels_str: String = ticks.iter()
        .map(|(_, l)| l.clone())
        .collect::<Vec<_>>()
        .join(",");

    // Build y-axis tick positions and labels dynamically
    let ytick_str: String = (1..=n_groups)
        .map(|y| y.to_string())
        .collect::<Vec<_>>()
        .join(",");
    // yticklabels must be in ascending y order (y=1 first)
    let mut labels_by_y: Vec<(usize, String)> = groups.iter()
        .map(|g| (g.y_pos, g.ytick_label.clone()))
        .collect();
    labels_by_y.sort_by_key(|(y, _)| *y);
    let yticklabels_inner: String = labels_by_y.iter()
        .map(|(_, l)| format!("      {}", l))
        .collect::<Vec<_>>()
        .join(",\n");

    // Compute separator y-position: between the lowest IPA group and the highest non-IPA group
    let ipa_min_y = groups.iter().filter(|g| g.is_ipa).map(|g| g.y_pos).min();
    let non_ipa_max_y = groups.iter().filter(|g| !g.is_ipa).map(|g| g.y_pos).max();
    let separator_y: Option<f64> = match (ipa_min_y, non_ipa_max_y) {
        (Some(ipa_y), Some(non_ipa_y)) if ipa_y > non_ipa_y => {
            Some((ipa_y as f64 + non_ipa_y as f64) / 2.0)
        }
        _ => None,
    };

    let ymax_f = n_groups as f64 + 0.5;

    // --- Figure (single tikzpicture, no minipage wrappers) ---
    let has_cov = cov.is_some();
    writeln!(s, "\\begin{{figure}}[t]").unwrap();
    writeln!(s, "  \\centering").unwrap();
    writeln!(s, "  \\begin{{tikzpicture}}").unwrap();
    writeln!(s, "  \\begin{{axis}}[").unwrap();
    if has_cov {
        writeln!(s, "    width=0.82\\columnwidth,").unwrap();
    } else {
        writeln!(s, "    width=0.95\\columnwidth,").unwrap();
    }
    writeln!(s, "    height=5.5cm,").unwrap();
    writeln!(s, "    boxplot/draw direction=x,").unwrap();
    writeln!(s, "    % X-axis: overpermissioning factor in log10 space").unwrap();
    writeln!(s, "    xlabel={{Overpermissioning factor (log-scale)}},").unwrap();
    writeln!(s, "    xlabel style={{font=\\small}},").unwrap();
    writeln!(s, "    xticklabel style={{font=\\small}},").unwrap();
    writeln!(s, "    xtick={{{}}},", xtick_str).unwrap();
    writeln!(s, "    xticklabels={{{}}},", xticklabels_str).unwrap();
    writeln!(s, "    xmin={:.2},", xmin).unwrap();
    writeln!(s, "    xmax={:.2},", xmax).unwrap();
    writeln!(s, "    % Y-axis: group labels (dynamically generated)").unwrap();
    writeln!(s, "    ytick={{{}}},", ytick_str).unwrap();
    writeln!(s, "    yticklabels={{").unwrap();
    writeln!(s, "{}}},", yticklabels_inner).unwrap();
    writeln!(s, "    yticklabel style={{font=\\small}},").unwrap();
    writeln!(s, "    ymin=0.5, ymax={:.1},", ymax_f).unwrap();
    writeln!(s, "    boxplot/box extend=0.38,").unwrap();
    writeln!(s, "    xmajorgrids=true,").unwrap();
    writeln!(s, "    grid style={{dashed,gray!20}},").unwrap();
    writeln!(s, "    clip=false,").unwrap();
    writeln!(s, "  ]").unwrap();
    writeln!(s).unwrap();

    // --- Baseline and shading ---
    writeln!(s, "  % ── Minimal policy baseline (x=0 in log10 = 1× linear) ──").unwrap();
    writeln!(s, "  \\draw[dashed, thick, black!50] (axis cs:0,0.5) -- (axis cs:0,{:.1});", ymax_f).unwrap();
    writeln!(s, "  \\node[anchor=south, font=\\scriptsize, text=black!60] at (axis cs:0,{:.2}) {{$1{{\\times}}$\\,min}};", ymax_f + 0.05).unwrap();
    writeln!(s).unwrap();
    writeln!(s, "  % ── Shaded region: underpermissioned (left of 1×) ──").unwrap();
    writeln!(s, "  \\fill[red!5] (axis cs:{:.2},0.5) rectangle (axis cs:0,{:.1});", xmin, ymax_f).unwrap();
    writeln!(s).unwrap();
    // --- Separator between IPA group and LLM groups (only if both exist) ---
    if let Some(sep_y) = separator_y {
        writeln!(s, "  % ── Separator between IPA group and LLM groups ──").unwrap();
        if has_cov {
            writeln!(s, "  \\draw[thin, black!30] (axis cs:{:.2},{:.1}) -- (axis cs:3.12,{:.1});", xmin, sep_y, sep_y).unwrap();
        } else {
            writeln!(s, "  \\draw[thin, black!30] (axis cs:{:.2},{:.1}) -- (axis cs:{:.2},{:.1});", xmin, sep_y, xmax, sep_y).unwrap();
        }
        writeln!(s).unwrap();
    }

    // --- Boxplot bodies ---
    for (g, st_opt) in groups.iter().zip(stats.iter()) {
        writeln!(s, "  % ══════════════════════════════════════════════════════").unwrap();
        let n_label = st_opt.as_ref().map_or(0, |st| st.n);
        writeln!(s, "  % Group {} (y={}): {} — n={}", g.y_pos, g.y_pos, g.label, n_label).unwrap();
        match st_opt {
            Some(st) => {
                writeln!(s, "  % lw={:.6} q1={:.6} med={:.6} q3={:.6} uw={:.6}",
                    st.lower_whisker, st.q1, st.median, st.q3, st.upper_whisker).unwrap();
                if st.outliers.is_empty() {
                    writeln!(s, "  % no outliers (all within whiskers)").unwrap();
                } else {
                    let ol_strs: Vec<String> = st.outliers.iter().map(|v| format!("{:.6}", v)).collect();
                    writeln!(s, "  % outlier: {}", ol_strs.join(", ")).unwrap();
                }
                writeln!(s, "  % ══════════════════════════════════════════════════════").unwrap();
                writeln!(s, "  \\addplot[").unwrap();
                writeln!(s, "    boxplot prepared={{").unwrap();
                writeln!(s, "      draw position={},", g.y_pos).unwrap();
                writeln!(s, "      lower whisker={:.6},", st.lower_whisker).unwrap();
                writeln!(s, "      lower quartile={:.6},", st.q1).unwrap();
                writeln!(s, "      median={:.6},", st.median).unwrap();
                writeln!(s, "      upper quartile={:.6},", st.q3).unwrap();
                writeln!(s, "      upper whisker={:.6},", st.upper_whisker).unwrap();
                writeln!(s, "    }},").unwrap();
                writeln!(s, "    color={}, fill={}, fill opacity={:.2},", g.colour, g.colour, g.fill_opacity).unwrap();
                writeln!(s, "    line width=0.8pt,").unwrap();
                writeln!(s, "  ] coordinates {{}};").unwrap();

                // Outliers as separate marks
                if !st.outliers.is_empty() {
                    let label_short = if g.label.len() > 20 { &g.label[..20] } else { &g.label };
                    for ol in &st.outliers {
                        writeln!(s, "  % {} outlier ({:.2}×)", label_short, 10_f64.powf(*ol)).unwrap();
                        writeln!(s, "  \\addplot[only marks, mark=*, mark size=1.2pt, color={}]", g.colour).unwrap();
                        writeln!(s, "    coordinates {{({:.6},{})}};", ol, g.y_pos).unwrap();
                    }
                }
            }
            None => {
                writeln!(s, "  % ══════════════════════════════════════════════════════").unwrap();
                writeln!(s, "  % (all values ≤ 0 — no finite log10 data; box omitted)").unwrap();
            }
        }
        writeln!(s).unwrap();
    }

    // --- Mean markers (diamond) ---
    writeln!(s, "  % ── Mean markers (diamond ◆) — plotted at log10(arithmetic mean) ──").unwrap();
    for (g, st_opt) in groups.iter().zip(stats.iter()) {
        if let Some(st) = st_opt {
            let colour_dark = if g.colour == "wongBlack" {
                "black!80".to_string()
            } else {
                format!("{}!80!black", g.colour)
            };
            writeln!(s, "  % {}: {:.2}×  → log10 = {:.4}",
                g.label, st.arith_mean_linear, st.mean_log10).unwrap();
            writeln!(s, "  \\addplot[only marks, mark=diamond*, mark size=2pt, color={}, fill={}]",
                colour_dark, g.colour).unwrap();
            writeln!(s, "    coordinates {{({:.4},{})}};", st.mean_log10, g.y_pos).unwrap();
        }
    }
    writeln!(s).unwrap();

    // --- Annotations: mean value (right of upper whisker or outlier) ---
    writeln!(s, "  % ── Annotations: mean value (right of upper whisker or outlier) ──").unwrap();
    for (g, st_opt) in groups.iter().zip(stats.iter()) {
        if let Some(st) = st_opt {
            let colour_dark = if g.colour == "wongBlack" {
                "black!80".to_string()
            } else {
                format!("{}!80!black", g.colour)
            };
            let annotation = format_mean_annotation(st.arith_mean_linear);
            // Position annotation to the right of the rightmost point
            let rightmost = st.upper_whisker
                .max(st.outliers.iter().copied().fold(f64::NEG_INFINITY, f64::max));
            let annot_x = rightmost + 0.02;

            if g.y_pos == 1 {
                // Managed: place annotation above the box
                writeln!(s, "  \\node[anchor=south, font=\\scriptsize, text={}]", colour_dark).unwrap();
                writeln!(s, "    at (axis cs:{:.4},{:.2}) {{{}}};", st.mean_log10, g.y_pos as f64 + 0.35, annotation).unwrap();
            } else {
                writeln!(s, "  \\node[anchor=west, font=\\scriptsize, text={}]", colour_dark).unwrap();
                writeln!(s, "    at (axis cs:{:.3},{}) {{\\,{}}};", annot_x, g.y_pos, annotation).unwrap();
            }
        }
    }
    writeln!(s).unwrap();

    // --- Coverage/Precision/F1 as TikZ nodes inside the axis (when coverage data available) ---
    if has_cov {
        render_coverage_nodes(s, cov.unwrap(), &groups, ymax_f, xmax);
    }

    // --- Close axis/tikzpicture ---
    writeln!(s, "  \\end{{axis}}").unwrap();
    writeln!(s, "  \\end{{tikzpicture}}").unwrap();

    // --- Caption and label ---
    writeln!(s, "  \\caption{{").unwrap();
    writeln!(s, "    Overpermissioning factor (number of IAM actions in the generated policy").unwrap();
    writeln!(s, "    divided by the number in the minimal policy) for {}~benchmarks,", report.successful_runs).unwrap();
    writeln!(s, "    aggregated across prompts and settings. IPA results are shown per language.").unwrap();
    writeln!(s, "    LLM-based methods and \\texttt{{iamfast}} are aggregated across languages.").unwrap();
    writeln!(s, "    The dashed line marks the minimal-policy baseline;").unwrap();
    writeln!(s, "    values to the left indicate missing permissions.").unwrap();
    writeln!(s, "    The vertical line inside each box is the median; the diamond~($\\blacklozenge$) marks the mean,").unwrap();
    writeln!(s, "    whose value is annotated next to each box.").unwrap();
    if has_cov {
        writeln!(s, "    The right panel shows coverage (fraction of required actions present)").unwrap();
        writeln!(s, "    and precision (fraction of generated actions that are required).").unwrap();
    }
    writeln!(s, "  }}").unwrap();
    writeln!(s, "  \\label{{fig:overpermission_detail}}").unwrap();
    writeln!(s, "\\end{{figure}}").unwrap();
}

// ---------------------------------------------------------------------------
// Figure 2: Summary statistics table (booktabs, single-column table)
//
// Required packages: booktabs
// Uses single-column table (narrow after removing Q1/Q3/AA/Val columns).
// Columns: Policy, Mean, Med., Std, Min, Max.
// ---------------------------------------------------------------------------

fn render_stats_table(report: &AggregateReport, label_prefix: &str) -> String {
    let mut s = String::new();

    writeln!(s, "% Summary statistics table").unwrap();
    writeln!(s, "% Generated by iac-paper-figures").unwrap();
    writeln!(s, "% Requires: booktabs").unwrap();
    writeln!(s, "% Uses single-column table for IEEEtran compatibility.").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "\\begin{{table}}[t]").unwrap();
    writeln!(s, "  \\centering").unwrap();
    writeln!(s, "  \\setlength{{\\tabcolsep}}{{3pt}}").unwrap();
    writeln!(s, "  \\caption{{").unwrap();
    writeln!(s, "    Overpermissioning factors compared to the minimal baseline policy.").unwrap();
    writeln!(s, "  }}").unwrap();
    writeln!(s, "  \\label{{{label_prefix}:stats_table}}").unwrap();
    writeln!(s, "  \\small").unwrap();

    writeln!(s, "  \\begin{{tabular}}{{lrrrrr}}").unwrap();
    writeln!(s, "    \\toprule").unwrap();
    writeln!(s, "    \\textbf{{Policy}} & \\textbf{{Mean}} & \\textbf{{Med.}} & \\textbf{{Std}} & \\textbf{{Min}} & \\textbf{{Max}} \\\\").unwrap();
    writeln!(s, "    \\midrule").unwrap();

    // Per-language Autopilot rows
    for la in &report.languages {
        let st = &la.ratio_vs_minimal;
        write!(s, "    Autopilot ({}) ", short_lang_label(&la.language)).unwrap();
        write_stats_row_compact(&mut s, st);
        if la.generation_failures > 0 {
            writeln!(s, "    % Note: {} generation failure(s) excluded from stats", la.generation_failures).unwrap();
        }
    }

    // Per-experiment LLM rows (sorted by tag via BTreeMap)
    for (tag, aggregates) in &report.llm_experiment_aggregates {
        if aggregates.is_empty() {
            continue;
        }
        writeln!(s, "    \\addlinespace").unwrap();
        for la in aggregates {
            let st = &la.ratio_vs_minimal;
            write!(s, "    {} ({}) ", latex_escape(tag), short_lang_label(&la.language)).unwrap();
            write_stats_row_compact(&mut s, st);
            if la.generation_failures > 0 {
                writeln!(s, "    % Note: {} {} generation failure(s) excluded from stats", la.generation_failures, tag).unwrap();
            }
        }
    }

    // Per-language iamfast rows
    if !report.iamfast_languages.is_empty() {
        writeln!(s, "    \\addlinespace").unwrap();
        for la in &report.iamfast_languages {
            let st = &la.ratio_vs_minimal;
            write!(s, "    iamfast ({}) ", short_lang_label(&la.language)).unwrap();
            write_stats_row_compact(&mut s, st);
            if la.generation_failures > 0 {
                writeln!(s, "    % Note: {} iamfast generation failure(s) excluded from stats", la.generation_failures).unwrap();
            }
        }
    }

    writeln!(s, "    \\addlinespace").unwrap();

    // Managed row
    let sc = &report.managed_vs_minimal_ratio;
    write!(s, "    Managed ").unwrap();
    write_stats_row_compact(&mut s, sc);

    writeln!(s, "    \\bottomrule").unwrap();
    writeln!(s, "  \\end{{tabular}}").unwrap();
    writeln!(s, "\\end{{table}}").unwrap();

    s
}

/// Compact stats row: Mean, Med., Std, Min, Max (no Q1/Q3/AA/Val).
fn write_stats_row_compact(s: &mut String, st: &Stats) {
    writeln!(
        s,
        "& {:.2} & {:.2} & {:.2} & {:.2} & {:.2} \\\\",
        st.mean, st.median, st.std_dev, st.min, st.max
    ).unwrap();
}

// ---------------------------------------------------------------------------
// Figure 3: Per-run validation results — compact 2×2 language-grid table
//
// IEEEtran single-column compatible:
//   - Single `table` (not table*) with one tabular.
//   - Each cell is a 2×2 grid encoding four languages:
//       Py Go / Jv TS
//     rendered via custom LaTeX macros \ipaq, \langq, \iamq.
//   - IPA and iamfast are deterministic (binary: 0 or 5).
//   - LLM experiments show pass count out of N trials (0–N).
//   - iamfast only supports a subset of languages; unsupported = N/A.
//   - Column headers are rotated 70° for compactness.
//
// Column layout:
//   1: run#  2: IPA  3-5: LLM(bare/wild/res*)  6-8: LLM_CTX(bare/wild/res*)
//   9: iamfast
//
// Required packages: booktabs, xcolor, tikz
// Required preamble macros: \ipaq, \langq, \iamq, \legsqn, \legsq
//   (colour squares sq0–sq5, sqna defined via \definecolor + \tikz)
// ---------------------------------------------------------------------------

/// The fixed language order used in the 2×2 grid cells.
/// Position: top-left=Py, top-right=Go, bottom-left=Jv, bottom-right=TS.
const GRID_LANGS: [&str; 4] = ["python", "go", "java", "typescript"];

/// The languages supported by iamfast (subset of GRID_LANGS).
/// Order matches the \iamq macro arguments.
const IAMFAST_LANGS: [&str; 2] = ["go", "java"];

/// The scenario groups and their experiment-tag → sub-column mappings.
/// Each group has three variants: bare, wildcards, resource-star.
/// The tags must match the keys in `RunSummary::llm_experiment_summaries`.
const SCENARIO_GROUPS: [(&str, &str, [&str; 3]); 2] = [
    // (group header for \multicolumn, LaTeX label, [bare_tag, wildcards_tag, resource-star_tag])
    ("\\textbf{LLM}",                              "LLM",      ["LLM/bare", "LLM/wildcards", "LLM/resource-star"]),
    ("\\textbf{LLM\\textsubscript{CTX}}",           "LLM_CTX",  ["CTX-LLM/bare", "CTX-LLM/wildcards", "CTX-LLM/resource-star"]),
];

/// Sub-column headers for the three variants within each scenario group.
const VARIANT_HEADERS: [&str; 3] = ["bare", "wild", "Res\\,{$\\ast$}"];

fn render_raw_data_table(report: &AggregateReport, label_prefix: &str) -> String {
    let mut s = String::new();

    let has_iamfast = !report.iamfast_languages.is_empty();

    // Filter SCENARIO_GROUPS to only include groups whose prefix has data
    let active_groups: Vec<(&str, &str, [&str; 3])> = SCENARIO_GROUPS.iter()
        .filter(|(_header, _label, tags)| {
            // A group is active if any of its tags appear as keys in llm_experiment_aggregates
            tags.iter().any(|tag| {
                report.llm_experiment_aggregates.get(*tag)
                    .map(|aggs| !aggs.is_empty())
                    .unwrap_or(false)
            })
        })
        .cloned()
        .collect();
    let n_groups = active_groups.len();

    // ── Preamble comment ────────────────────────────────────────────────────
    writeln!(s, "% Per-run validation results table — compact 2x2 language-grid version").unwrap();
    writeln!(s, "% Each cell is a 2x2 grid: Py (top-left), Go (top-right), Java (bottom-left), TS (bottom-right)").unwrap();
    writeln!(s, "% Column layout: # | IPA | <active LLM groups> | iamfast").unwrap();
    writeln!(s, "%").unwrap();
    writeln!(s, "% Generated by iac-paper-figures").unwrap();
    writeln!(s, "% Requires: booktabs, xcolor, tikz").unwrap();
    writeln!(s, "% Requires preamble macros: \\ipaq, \\langq, \\iamq, \\legsqn, \\legsq").unwrap();
    writeln!(s, "%").unwrap();
    writeln!(s, "% Sequential run IDs:").unwrap();
    for (seq, run) in report.runs.iter().enumerate() {
        writeln!(s, "%   {:02} -> {}", seq + 1, run.run_name).unwrap();
    }
    writeln!(s).unwrap();

    // Determine the number of trials from the first run's first LLM experiment
    // (needed for caption legend and data rows)
    let n_trials = report.runs.first()
        .and_then(|run| {
            run.llm_experiment_summaries.values().next()
                .and_then(|sums| sums.first())
                .map(|ls| ls.llm_trials.len())
        })
        .unwrap_or(5);

    // ── table wrapper ────────────────────────────────────────────────────────
    writeln!(s, "\\begin{{table}}[t]").unwrap();
    writeln!(s, "  \\centering").unwrap();
    writeln!(s, "  \\setlength{{\\tabcolsep}}{{2pt}}").unwrap();
    writeln!(s, "  \\caption{{Per-run results.").unwrap();
    writeln!(s, "    Each cell is a 2$\\times$2 grid encoding four languages:").unwrap();
    writeln!(s, "    \\raisebox{{0.2em}}{{\\tiny Python}}\\,\\raisebox{{0.2em}}{{\\tiny Go}} /").unwrap();
    writeln!(s, "    \\raisebox{{-0.1em}}{{\\tiny Java}}\\,\\raisebox{{-0.1em}}{{\\tiny TS}}.").unwrap();
    writeln!(s, "    LLM experiments are repeated {} times. Color indicates pass rate:", n_trials).unwrap();
    for i in (0..=n_trials).rev() {
        if i == 0 {
            writeln!(s, "    {{\\protect\\legsqn{{sq{i}}}{{{i}}}}}\\,{i}/{n_trials}.").unwrap();
        } else {
            writeln!(s, "    {{\\protect\\legsqn{{sq{i}}}{{{i}}}}}\\,{i}/{n_trials},").unwrap();
        }
    }
    writeln!(s, "    IPA and iamfast are deterministic, experiments are run once:").unwrap();
    writeln!(s, "    {{\\protect\\legsqn{{sq{n_trials}}}{{\\checkmark}}}}\\,=\\,pass,").unwrap();
    writeln!(s, "     {{\\protect\\legsqn{{sq0}}{{$\\times$}}}}\\,=\\,fail.").unwrap();
    if has_iamfast {
        let iamfast_supported: Vec<String> = report.iamfast_languages.iter()
            .map(|la| capitalize(&la.language))
            .collect();
        writeln!(s, "    iamfast supports only {}", iamfast_supported.join(" and ")).unwrap();
        writeln!(s, "    ({{\\protect\\legsq{{sqna}}}}\\,=\\,N/A).").unwrap();
    }
    writeln!(s, "  }}").unwrap();
    writeln!(s, "  \\label{{{label_prefix}:raw_data}}").unwrap();
    writeln!(s, "  \\small").unwrap();
    writeln!(s).unwrap();

    // Column spec: @{}r Q  <3 Q per active group>  Q@{}
    // Total Q columns = 1 (IPA) + 3*n_groups (LLM) + 1 (iamfast if present)
    writeln!(s, "  \\newcolumntype{{Q}}{{w{{c}}{{1.7em}}}}").unwrap();
    {
        let mut col_spec = String::from("@{}r Q ");
        for _ in 0..n_groups {
            col_spec.push_str("QQQ ");
        }
        if has_iamfast {
            col_spec.push_str("Q");
        }
        col_spec.push_str("@{}");
        writeln!(s, "  \\begin{{tabular}}{{{col_spec}}}").unwrap();
    }
    writeln!(s, "    \\toprule").unwrap();

    // ── First header row: multicolumn spans for the active scenario groups ──
    write!(s, "    ").unwrap();
    write!(s, "& ").unwrap();                    // col 2 (IPA) — empty
    for (header, _label, _tags) in &active_groups {
        write!(s, "& \\multicolumn{{3}}{{c}}{{{header}}} ").unwrap();
    }
    if has_iamfast {
        writeln!(s, "& \\\\").unwrap();          // iamfast — empty in first row
    } else {
        writeln!(s, "\\\\").unwrap();
    }
    // cmidrule spans: col 3-5 for first group, 6-8 for second, etc.
    {
        let mut rules = Vec::new();
        for i in 0..n_groups {
            let start = 3 + i * 3;
            let end = start + 2;
            rules.push(format!("\\cmidrule(lr){{{start}-{end}}}"));
        }
        writeln!(s, "    {}", rules.join(" ")).unwrap();
    }

    // ── Second header row: rotated sub-column labels ────────────────────────
    write!(s, "    \\textbf{{\\#}}").unwrap();
    write!(s, "\n      & \\rotatebox{{70}}{{\\textbf{{IPA}}}}").unwrap();
    for _ in 0..n_groups {
        for var_header in &VARIANT_HEADERS {
            write!(s, "\n      & \\rotatebox{{70}}{{\\textbf{{{var_header}}}}}").unwrap();
        }
    }
    if has_iamfast {
        write!(s, "\n      & \\rotatebox{{70}}{{\\textbf{{iamf.}}}}").unwrap();
    }
    writeln!(s, " \\\\").unwrap();
    writeln!(s, "    \\midrule").unwrap();

    // ── Accumulators for the aggregate Pass row ─────────────────────────────
    let mut ipa_pass_count: usize = 0;
    // LLM groups: sum of all \langq cell values (each 0–n_trials, 4 langs per cell)
    // Index: [group_idx][variant_idx] — n_groups × 3 variants
    let mut llm_sums: Vec<[usize; 3]> = vec![[0; 3]; n_groups];
    let mut iamfast_pass_count: usize = 0;

    // ── Data rows ──────────────────────────────────────────────────────────
    let n_runs = report.runs.len();
    for (seq, run) in report.runs.iter().enumerate() {
        let is_last = seq + 1 == n_runs;

        writeln!(s, "    % Row {:02} — {}", seq + 1, run.run_name).unwrap();
        write!(s, "    \\texttt{{{:02}}}", seq + 1).unwrap();

        // IPA (autopilot) cell — \ipaq{Py}{Go}{Java}{TS}
        let ipa_vals: Vec<usize> = GRID_LANGS.iter().map(|lang| {
            run.language_summaries.iter()
                .find(|ls| ls.language == *lang)
                .map(|ls| {
                    if ls.policy_generated && ls.validation_success == Some(true) {
                        n_trials
                    } else {
                        0
                    }
                })
                .unwrap_or(0)
        }).collect();
        for v in &ipa_vals {
            if *v == n_trials { ipa_pass_count += 1; }
        }
        write!(s, "\n      & \\ipaq{{{}}}{{{}}}{{{}}}{{{}}}", ipa_vals[0], ipa_vals[1], ipa_vals[2], ipa_vals[3]).unwrap();

        // LLM experiment group cells — n_groups × 3 variants
        for (group_idx, (_header, _label, tags)) in active_groups.iter().enumerate() {
            for (var_idx, tag) in tags.iter().enumerate() {
                let summaries = run.llm_experiment_summaries.get(*tag);
                let vals: Vec<usize> = GRID_LANGS.iter().map(|lang| {
                    summaries
                        .and_then(|sums| sums.iter().find(|ls| ls.language == *lang))
                        .map(|ls| {
                            if !ls.llm_trials.is_empty() {
                                ls.llm_trials.iter().filter(|t| t.validation_success).count()
                            } else if ls.policy_generated {
                                if ls.validation_success.unwrap_or(false) { n_trials } else { 0 }
                            } else {
                                0
                            }
                        })
                        .unwrap_or(0)
                }).collect();
                let cell_sum: usize = vals.iter().sum();
                llm_sums[group_idx][var_idx] += cell_sum;
                write!(s, "\n      & \\langq{{{}}}{{{}}}{{{}}}{{{}}}", vals[0], vals[1], vals[2], vals[3]).unwrap();
            }
        }

        // iamfast cell — \iamq{Go}{Java}
        if has_iamfast {
            let iamfast_vals: Vec<usize> = IAMFAST_LANGS.iter().map(|lang| {
                run.iamfast_language_summaries.iter()
                    .find(|ls| ls.language == *lang)
                    .map(|ls| {
                        if ls.policy_generated && ls.validation_success.unwrap_or(false) {
                            n_trials
                        } else {
                            0
                        }
                    })
                    .unwrap_or(0)
            }).collect();
            for v in &iamfast_vals {
                if *v == n_trials { iamfast_pass_count += 1; }
            }
            if is_last {
                writeln!(s, "\n      & \\iamq{{{}}}{{{}}} \\\\", iamfast_vals[0], iamfast_vals[1]).unwrap();
            } else {
                writeln!(s, "\n      & \\iamq{{{}}}{{{}}} \\\\[3pt]", iamfast_vals[0], iamfast_vals[1]).unwrap();
            }
        } else if is_last {
            writeln!(s, " \\\\").unwrap();
        } else {
            writeln!(s, " \\\\[3pt]").unwrap();
        }
    }

    // ── Aggregate Pass row ──────────────────────────────────────────────────
    writeln!(s, "    \\midrule").unwrap();
    write!(s, "    \\textbf{{Pass}}").unwrap();

    // IPA percentage: pass cells / (n_runs × 4 languages)
    let ipa_denom = n_runs * 4;
    let ipa_pct = if ipa_denom > 0 {
        ((ipa_pass_count as f64 / ipa_denom as f64) * 100.0).round() as usize
    } else { 0 };
    write!(s, "\n      & \\textbf{{{}\\%}}", ipa_pct).unwrap();

    // LLM group percentages: sum / (n_runs × 4 languages × n_trials)
    let llm_denom = n_runs * 4 * n_trials;
    for group_idx in 0..n_groups {
        for var_idx in 0..3 {
            let pct = if llm_denom > 0 {
                ((llm_sums[group_idx][var_idx] as f64 / llm_denom as f64) * 100.0).round() as usize
            } else { 0 };
            write!(s, "\n      & \\textbf{{{}\\%}}", pct).unwrap();
        }
    }

    // iamfast percentage: pass cells / (n_runs × 2 languages)
    if has_iamfast {
        let iamfast_denom = n_runs * 2;
        let iamfast_pct = if iamfast_denom > 0 {
            ((iamfast_pass_count as f64 / iamfast_denom as f64) * 100.0).round() as usize
        } else { 0 };
        write!(s, "\n      & \\textbf{{{}\\%}}", iamfast_pct).unwrap();
    }
    writeln!(s, " \\\\").unwrap();

    writeln!(s, "    \\bottomrule").unwrap();
    writeln!(s, "  \\end{{tabular}}").unwrap();
    writeln!(s, "\\end{{table}}").unwrap();

    s
}

// ---------------------------------------------------------------------------
// Experiment tag → short column header for the compact raw-data table
// ---------------------------------------------------------------------------

/// Convert an experiment tag like "CTX-LLM/bare" to a short column header
/// like "CTX/bare" for the compact raw-data table.
#[allow(dead_code)]
fn short_experiment_header(tag: &str) -> String {
    let (prefix_raw, variant) = match tag.split_once('/') {
        Some((p, v)) => (p, v),
        None => return latex_escape(tag),
    };
    let prefix = if let Some(p) = prefix_raw.strip_suffix("-LLM") {
        p.to_string()
    } else {
        prefix_raw.to_string()
    };
    let short_variant = match variant {
        "bare" => "bare".to_string(),
        "resource-star" => "res\\textsuperscript{*}".to_string(),
        "wildcards" => "wild".to_string(),
        other => latex_escape(other),
    };
    format!("{}/{}", prefix, short_variant)
}

// ---------------------------------------------------------------------------
// Figure 4: Token usage table
// ---------------------------------------------------------------------------

fn render_token_usage_table(report: &AggregateReport, label_prefix: &str) -> String {
    let has_any_tokens = report.llm_experiment_aggregates.values()
        .any(|langs| langs.iter().any(|la| la.total_tokens.is_some()));

    if !has_any_tokens {
        return String::new();
    }

    let mut s = String::new();

    writeln!(s, "% Token usage statistics table").unwrap();
    writeln!(s, "% Generated by iac-paper-figures").unwrap();
    writeln!(s, "% Requires: booktabs, siunitx").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "\\begin{{table}}[t]").unwrap();
    writeln!(s, "  \\centering").unwrap();
    writeln!(s, "  \\setlength{{\\tabcolsep}}{{3pt}}").unwrap();
    writeln!(s, "  \\caption{{").unwrap();
    writeln!(s, "    Token usage per run (mean across all trials within each run).").unwrap();
    writeln!(s, "    Input tokens correspond to the prompt; output tokens to the LLM completion.").unwrap();
    writeln!(s, "  }}").unwrap();
    writeln!(s, "  \\label{{{label_prefix}:token_usage}}").unwrap();
    writeln!(s, "  \\small").unwrap();
    writeln!(s, "  \\begin{{tabular}}{{l r r r r r r}}").unwrap();
    writeln!(s, "    \\toprule").unwrap();
    writeln!(s, "    \\textbf{{Approach (Lang)}} & \\textbf{{Mean}} & \\textbf{{Med.}} & \\textbf{{Std}} & \\textbf{{Min}} & \\textbf{{Max}} & \\textbf{{Type}} \\\\").unwrap();
    writeln!(s, "    \\midrule").unwrap();

    let write_group = |s: &mut String, group_label: &str, langs: &[iac_benchmarker::aggregate::LanguageAggregate]| {
        let mut first = true;
        for la in langs {
            if let Some(ref total) = la.total_tokens {
                if first {
                    writeln!(s, "    \\addlinespace").unwrap();
                    first = false;
                }
                write!(s, "    {} ({}) ", group_label, short_lang_label(&la.language)).unwrap();
                writeln!(s, "& {:.0} & {:.0} & {:.0} & {:.0} & {:.0} & Total \\\\",
                    total.mean, total.median, total.std_dev, total.min, total.max).unwrap();
                if let Some(ref input) = la.input_tokens {
                    write!(s, "    \\quad ").unwrap();
                    writeln!(s, "& {:.0} & {:.0} & {:.0} & {:.0} & {:.0} & Input \\\\",
                        input.mean, input.median, input.std_dev, input.min, input.max).unwrap();
                }
                if let Some(ref output) = la.output_tokens {
                    write!(s, "    \\quad ").unwrap();
                    writeln!(s, "& {:.0} & {:.0} & {:.0} & {:.0} & {:.0} & Output \\\\",
                        output.mean, output.median, output.std_dev, output.min, output.max).unwrap();
                }
            }
        }
    };

    for (tag, aggregates) in &report.llm_experiment_aggregates {
        if aggregates.iter().any(|la| la.total_tokens.is_some()) {
            write_group(&mut s, &latex_escape(tag), aggregates);
        }
    }

    writeln!(s, "    \\bottomrule").unwrap();
    writeln!(s, "  \\end{{tabular}}").unwrap();
    writeln!(s, "\\end{{table}}").unwrap();

    s
}

// ---------------------------------------------------------------------------
// Figure 5: Token cost summary — inline sentence for paper body
//
// Computes the grand mean (± std) of input/output tokens across all LLM
// experiments and languages, then estimates USD cost using Claude Opus 4.6
// pricing: $5.00/1M input tokens, $25.00/1M output tokens.
// ---------------------------------------------------------------------------

fn render_token_cost_summary(report: &AggregateReport) -> String {
    // Collect all per-experiment, per-language mean token stats.
    let mut input_means: Vec<f64> = Vec::new();
    let mut output_means: Vec<f64> = Vec::new();

    for (_tag, aggregates) in &report.llm_experiment_aggregates {
        for la in aggregates {
            if let Some(ref input) = la.input_tokens {
                input_means.push(input.mean);
            }
            if let Some(ref output) = la.output_tokens {
                output_means.push(output.mean);
            }
        }
    }

    if input_means.is_empty() && output_means.is_empty() {
        return String::new();
    }

    let mean_of = |vals: &[f64]| -> f64 {
        vals.iter().sum::<f64>() / vals.len() as f64
    };
    let std_of = |vals: &[f64]| -> f64 {
        let m = mean_of(vals);
        let var = vals.iter().map(|v| (v - m).powi(2)).sum::<f64>() / vals.len() as f64;
        var.sqrt()
    };

    let grand_input_mean = mean_of(&input_means);
    let grand_input_std = std_of(&input_means);
    let grand_output_mean = mean_of(&output_means);
    let grand_output_std = std_of(&output_means);

    // Claude Opus 4.6 pricing (USD per 1M tokens).
    let input_price_per_1m: f64 = 5.00;
    let output_price_per_1m: f64 = 25.00;

    let cost_per_call = (grand_input_mean / 1_000_000.0) * input_price_per_1m
        + (grand_output_mean / 1_000_000.0) * output_price_per_1m;

    let mut s = String::new();

    writeln!(s, "% Token cost summary — inline sentence for paper body").unwrap();
    writeln!(s, "% Generated by iac-paper-figures").unwrap();
    writeln!(s, "% Grand mean computed across all LLM experiment tags and languages.").unwrap();
    writeln!(s, "% Pricing: Claude Opus 4.6 — $5.00/1M input, $25.00/1M output tokens.").unwrap();
    writeln!(s, "%").unwrap();
    writeln!(s, "% Grand input  mean: {:.0} (± {:.0})", grand_input_mean, grand_input_std).unwrap();
    writeln!(s, "% Grand output mean: {:.0} (± {:.0})", grand_output_mean, grand_output_std).unwrap();
    writeln!(s, "% Estimated cost per LLM call: ${:.4}", cost_per_call).unwrap();
    writeln!(s).unwrap();
    writeln!(
        s,
        "\\noindent\n\
         On average, a single LLM policy-generation call consumed \
         \\num{{{:.0}}} input tokens ($\\pm$\\num{{{:.0}}}) and \
         \\num{{{:.0}}} output tokens ($\\pm$\\num{{{:.0}}}). \
         At list pricing, \
         this corresponds to \\$\\num{{{:.4}}} per call, at time of writing.\n\
         \\endinput",
        grand_input_mean, grand_input_std,
        grand_output_mean, grand_output_std,
        cost_per_call,
    ).unwrap();
    writeln!(s).unwrap();

    s
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Capitalise the first letter of a string.
fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

/// Short display label for a language: "typescript" → "TS", others capitalised.
fn short_lang_label(lang: &str) -> String {
    match lang.to_lowercase().as_str() {
        "typescript" => "TS".to_string(),
        other => capitalize(other),
    }
}

/// Escape special LaTeX characters in a string.
fn latex_escape(s: &str) -> String {
    s.replace('&', "\\&")
        .replace('%', "\\%")
        .replace('$', "\\$")
        .replace('#', "\\#")
        .replace('_', "\\_")
        .replace('{', "\\{")
        .replace('}', "\\}")
        .replace('~', "\\textasciitilde{}")
        .replace('^', "\\textasciicircum{}")
        .replace('\\', "\\textbackslash{}")
}

// ---------------------------------------------------------------------------
// Coverage/Precision as TikZ nodes inside the axis environment
//
// Places two columns of data (Cov, Prec) as \node commands at axis cs
// coordinates beyond xmax, so each row is vertically aligned with its
// corresponding box-plot row.  Requires clip=false on the axis.
//
// F1 score is intentionally omitted: it treats precision and recall
// symmetrically, but in this domain missing a required IAM action causes
// application failure (catastrophic), while extra actions cause
// overpermissioning (undesirable but functional).  Coverage and precision
// shown separately are more informative.
//
// Column x-positions (in log10 axis coordinates):
//   Cov  = xmax + 0.22
//   Prec = xmax + 0.58
// Column headers at y = 9.5 (just above the top row).
// ---------------------------------------------------------------------------

fn render_coverage_nodes(
    s: &mut String,
    cov: &CoverageReport,
    groups: &[BoxplotGroup],
    ymax_f: f64,
    xmax: f64,
) {
    // Build a map of approach → languages for easy lookup
    let approach_map: BTreeMap<String, &Vec<LanguageCoverageAggregate>> = cov
        .aggregates
        .iter()
        .map(|a| (a.approach.clone(), &a.languages))
        .collect();

    // Helper: find a specific language within an approach
    let find_lang = |approach: &str, lang: &str| -> Option<&LanguageCoverageAggregate> {
        approach_map.get(approach).and_then(|langs| {
            langs.iter().find(|la| la.language.eq_ignore_ascii_case(lang))
        })
    };

    // Helper: pool all languages across all strategies with a given prefix
    let pool_llm = |prefix: &str| -> Option<(Stats, Stats)> {
        let mut cov_vals: Vec<f64> = Vec::new();
        let mut prec_vals: Vec<f64> = Vec::new();
        for (approach, langs) in &approach_map {
            if approach.starts_with(prefix) {
                for la in langs.iter() {
                    cov_vals.extend_from_slice(&la.coverage.values);
                    prec_vals.extend_from_slice(&la.precision.values);
                }
            }
        }
        if cov_vals.is_empty() {
            return None;
        }
        match (Stats::compute(&cov_vals), Stats::compute(&prec_vals)) {
            (Some(c), Some(p)) => Some((c, p)),
            _ => None,
        }
    };

    // Helper: pool all languages for a single-approach key (iamfast, Managed)
    let pool_approach = |approach: &str| -> Option<(Stats, Stats)> {
        let mut cov_vals: Vec<f64> = Vec::new();
        let mut prec_vals: Vec<f64> = Vec::new();
        if let Some(langs) = approach_map.get(approach) {
            for la in langs.iter() {
                cov_vals.extend_from_slice(&la.coverage.values);
                prec_vals.extend_from_slice(&la.precision.values);
            }
        }
        if cov_vals.is_empty() {
            return None;
        }
        match (Stats::compute(&cov_vals), Stats::compute(&prec_vals)) {
            (Some(c), Some(p)) => Some((c, p)),
            _ => None,
        }
    };

    // Column x-positions beyond xmax
    let col_cov  = xmax + 0.22;
    let col_prec = xmax + 0.58;

    struct CovRow {
        y_pos: usize,
        cov_mean: Option<f64>,
        prec_mean: Option<f64>,
    }

    // Build rows dynamically from the groups list
    let mut rows: Vec<CovRow> = Vec::new();
    for g in groups {
        let pair: Option<(f64, f64)> = if g.coverage_id.starts_with("ipa:") {
            let lang = &g.coverage_id[4..];
            find_lang("Autopilot", lang).map(|la| (la.coverage.mean, la.precision.mean))
        } else if g.coverage_id.starts_with("llm:") {
            let prefix = &g.coverage_id[4..];
            pool_llm(prefix).map(|(c, p)| (c.mean, p.mean))
        } else if g.coverage_id == "iamfast" {
            pool_approach("iamfast").map(|(c, p)| (c.mean, p.mean))
        } else if g.coverage_id == "managed" {
            pool_approach("Managed").map(|(c, p)| (c.mean, p.mean))
        } else {
            None
        };
        rows.push(CovRow {
            y_pos: g.y_pos,
            cov_mean: pair.map(|(c, _)| c),
            prec_mean: pair.map(|(_, p)| p),
        });
    }

    // Emit TikZ nodes
    writeln!(s).unwrap();
    writeln!(s, "  % ── Coverage / Precision table (TikZ nodes, aligned with box plots) ──").unwrap();
    writeln!(s, "  % F1 intentionally omitted: asymmetric costs make it misleading here.").unwrap();

    // Column headers just above the top row
    let header_y = ymax_f + 0.5;
    writeln!(s, "  \\node[anchor=south, font=\\scriptsize\\bfseries] at (axis cs:{:.2},{:.1}) {{Cov}};", col_cov, header_y).unwrap();
    writeln!(s, "  \\node[anchor=south, font=\\scriptsize\\bfseries] at (axis cs:{:.2},{:.1}) {{Prec}};", col_prec, header_y).unwrap();

    // Data rows
    for row in &rows {
        match (row.cov_mean, row.prec_mean) {
            (Some(c), Some(p)) => {
                writeln!(s, "  \\node[anchor=center, font=\\small] at (axis cs:{:.2},{}) {{{:.2}}};", col_cov, row.y_pos, c).unwrap();
                writeln!(s, "  \\node[anchor=center, font=\\small] at (axis cs:{:.2},{}) {{{:.2}}};", col_prec, row.y_pos, p).unwrap();
            }
            _ => {
                writeln!(s, "  \\node[anchor=center, font=\\small] at (axis cs:{:.2},{}) {{---}};", col_cov, row.y_pos).unwrap();
                writeln!(s, "  \\node[anchor=center, font=\\small] at (axis cs:{:.2},{}) {{---}};", col_prec, row.y_pos).unwrap();
            }
        }
    }
    writeln!(s).unwrap();
}

// ---------------------------------------------------------------------------
// Compact coverage column for the side-by-side minipage layout (legacy)
//
// Renders a small tabular (matching the box plot y-positions)
// showing coverage, precision, and F1 for each approach.
// Row order (top to bottom): IPA(Py), IPA(Go), IPA(Java), IPA(TS),
//   LLM_CTX, LLM, iamfast, Managed.
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn render_coverage_column(s: &mut String, cov: &CoverageReport) {
    // Build a map of approach → languages for easy lookup
    let approach_map: BTreeMap<String, &Vec<LanguageCoverageAggregate>> = cov
        .aggregates
        .iter()
        .map(|a| (a.approach.clone(), &a.languages))
        .collect();

    // Helper: find a specific language within an approach
    let find_lang = |approach: &str, lang: &str| -> Option<&LanguageCoverageAggregate> {
        approach_map.get(approach).and_then(|langs| {
            langs.iter().find(|la| la.language.eq_ignore_ascii_case(lang))
        })
    };

    // Helper: pool all languages across all strategies with a given prefix
    let pool_llm = |prefix: &str| -> Option<(Stats, Stats, Stats)> {
        let mut cov_vals: Vec<f64> = Vec::new();
        let mut prec_vals: Vec<f64> = Vec::new();
        let mut f1_vals: Vec<f64> = Vec::new();
        for (approach, langs) in &approach_map {
            if approach.starts_with(prefix) {
                for la in langs.iter() {
                    cov_vals.extend_from_slice(&la.coverage.values);
                    prec_vals.extend_from_slice(&la.precision.values);
                    f1_vals.extend_from_slice(&la.f1.values);
                }
            }
        }
        if cov_vals.is_empty() {
            return None;
        }
        match (Stats::compute(&cov_vals), Stats::compute(&prec_vals), Stats::compute(&f1_vals)) {
            (Some(c), Some(p), Some(f)) => Some((c, p, f)),
            _ => None,
        }
    };

    // Build 9 rows: (label, Option<(cov_mean, prec_mean, f1_mean)>)
    // Order: top-to-bottom in the table = y=9 down to y=1
    struct CovRow {
        cov_mean: f64,
        prec_mean: f64,
        f1_mean: f64,
    }

    let mut rows: Vec<Option<CovRow>> = Vec::new();

    // y=9: IPA (Python)
    rows.push(find_lang("Autopilot", "python").map(|la| CovRow {
        cov_mean: la.coverage.mean, prec_mean: la.precision.mean, f1_mean: la.f1.mean,
    }));
    // y=8: IPA (Go)
    rows.push(find_lang("Autopilot", "go").map(|la| CovRow {
        cov_mean: la.coverage.mean, prec_mean: la.precision.mean, f1_mean: la.f1.mean,
    }));
    // y=7: IPA (Java)
    rows.push(find_lang("Autopilot", "java").map(|la| CovRow {
        cov_mean: la.coverage.mean, prec_mean: la.precision.mean, f1_mean: la.f1.mean,
    }));
    // y=6: IPA (TS)
    rows.push(find_lang("Autopilot", "typescript").map(|la| CovRow {
        cov_mean: la.coverage.mean, prec_mean: la.precision.mean, f1_mean: la.f1.mean,
    }));
    // y=5: LLM_CTX
    rows.push(pool_llm("CTX-LLM/").map(|(c, p, f)| CovRow {
        cov_mean: c.mean, prec_mean: p.mean, f1_mean: f.mean,
    }));
    // y=4: LLM
    rows.push(pool_llm("LLM/").map(|(c, p, f)| CovRow {
        cov_mean: c.mean, prec_mean: p.mean, f1_mean: f.mean,
    }));
    // y=2: iamfast (pool all languages)
    rows.push({
        let mut cov_vals: Vec<f64> = Vec::new();
        let mut prec_vals: Vec<f64> = Vec::new();
        let mut f1_vals: Vec<f64> = Vec::new();
        if let Some(langs) = approach_map.get("iamfast") {
            for la in langs.iter() {
                cov_vals.extend_from_slice(&la.coverage.values);
                prec_vals.extend_from_slice(&la.precision.values);
                f1_vals.extend_from_slice(&la.f1.values);
            }
        }
        match (Stats::compute(&cov_vals), Stats::compute(&prec_vals), Stats::compute(&f1_vals)) {
            (Some(c), Some(p), Some(f)) => Some(CovRow {
                cov_mean: c.mean, prec_mean: p.mean, f1_mean: f.mean,
            }),
            _ => None,
        }
    });
    // y=1: Managed (may not exist in coverage report)
    rows.push({
        let mut cov_vals: Vec<f64> = Vec::new();
        let mut prec_vals: Vec<f64> = Vec::new();
        let mut f1_vals: Vec<f64> = Vec::new();
        if let Some(langs) = approach_map.get("Managed") {
            for la in langs.iter() {
                cov_vals.extend_from_slice(&la.coverage.values);
                prec_vals.extend_from_slice(&la.precision.values);
                f1_vals.extend_from_slice(&la.f1.values);
            }
        }
        match (Stats::compute(&cov_vals), Stats::compute(&prec_vals), Stats::compute(&f1_vals)) {
            (Some(c), Some(p), Some(f)) => Some(CovRow {
                cov_mean: c.mean, prec_mean: p.mean, f1_mean: f.mean,
            }),
            _ => None,
        }
    });

    // Render the compact tabular
    writeln!(s, "    \\small").unwrap();
    writeln!(s, "    \\setlength{{\\tabcolsep}}{{2pt}}").unwrap();
    writeln!(s, "    \\begin{{tabular}}{{@{{}}rrr@{{}}}}").unwrap();
    writeln!(s, "      \\toprule").unwrap();
    writeln!(s, "      \\textbf{{Cov}} & \\textbf{{Prec}} & \\textbf{{F1}} \\\\").unwrap();
    writeln!(s, "      \\midrule").unwrap();

    for (i, row_opt) in rows.iter().enumerate() {
        match row_opt {
            Some(r) => {
                writeln!(s, "      {:.2} & {:.2} & {:.2} \\\\",
                    r.cov_mean, r.prec_mean, r.f1_mean).unwrap();
            }
            None => {
                writeln!(s, "      --- & --- & --- \\\\").unwrap();
            }
        }
        // Add separator between IPA group (rows 0-3) and LLM groups (rows 4-6)
        if i == 3 {
            writeln!(s, "      \\addlinespace[2pt]").unwrap();
        }
    }

    writeln!(s, "      \\bottomrule").unwrap();
    writeln!(s, "    \\end{{tabular}}").unwrap();
}

// ---------------------------------------------------------------------------
// Figure 6: Coverage / Precision / F1 table
//
// Reads the coverage_report.json produced by iac-coverage-analyzer and
// renders a booktabs table showing per-approach, per-language aggregate
// coverage (recall), precision, and F1 scores.
//
// The table groups approaches into: Autopilot, LLM variants (pooled by
// scenario prefix), iamfast.  Each row shows one approach with columns
// for coverage, precision, and F1 (mean ± std).
// ---------------------------------------------------------------------------

fn render_coverage_table(cov: &CoverageReport, label_prefix: &str) -> String {
    let mut s = String::new();

    writeln!(s, "% Coverage / Precision / F1 table").unwrap();
    writeln!(s, "% Generated by iac-paper-figures from coverage_report.json").unwrap();
    writeln!(s, "% Requires: booktabs").unwrap();
    writeln!(s).unwrap();

    // Group approaches for a cleaner table layout.
    // We'll show: Autopilot per-language, then LLM groups pooled across
    // languages and strategies, then iamfast per-language.

    // First, build a map of approach → languages for easy lookup
    let approach_map: BTreeMap<String, &Vec<LanguageCoverageAggregate>> = cov
        .aggregates
        .iter()
        .map(|a| (a.approach.clone(), &a.languages))
        .collect();

    // Identify LLM scenario groups by prefix
    let llm_group_labels = [
        ("LLM/", "LLM"),
        ("CTX-LLM/", "$\\text{LLM}_{\\text{CTX}}$"),
    ];

    writeln!(s, "\\begin{{table}}[t]").unwrap();
    writeln!(s, "  \\centering").unwrap();
    writeln!(s, "  \\setlength{{\\tabcolsep}}{{3pt}}").unwrap();
    writeln!(s, "  \\caption{{").unwrap();
    writeln!(s, "    Coverage (recall), precision, and F1 scores for generated policies").unwrap();
    writeln!(s, "    compared to the minimal ground-truth policy.").unwrap();
    writeln!(s, "    Coverage measures the fraction of required actions present;").unwrap();
    writeln!(s, "    precision measures the fraction of generated actions that are required.").unwrap();
    writeln!(s, "    Values are mean $\\pm$ std across all runs.").unwrap();
    writeln!(s, "  }}").unwrap();
    writeln!(s, "  \\label{{{label_prefix}:coverage_table}}").unwrap();
    writeln!(s, "  \\small").unwrap();
    writeln!(s, "  \\begin{{tabular}}{{l r@{{\\,}}l r@{{\\,}}l r@{{\\,}}l}}").unwrap();
    writeln!(s, "    \\toprule").unwrap();
    writeln!(s, "    \\textbf{{Approach}} & \\multicolumn{{2}}{{c}}{{\\textbf{{Coverage}}}} & \\multicolumn{{2}}{{c}}{{\\textbf{{Precision}}}} & \\multicolumn{{2}}{{c}}{{\\textbf{{F1}}}} \\\\").unwrap();
    writeln!(s, "    \\midrule").unwrap();

    // --- Autopilot per-language ---
    if let Some(langs) = approach_map.get("Autopilot") {
        for la in langs.iter() {
            write!(s, "    Autopilot ({}) ", short_lang_label(&la.language)).unwrap();
            write_coverage_row(&mut s, &la.coverage, &la.precision, &la.f1);
        }
    }

    // --- LLM groups: pool all languages across all strategies with same prefix ---
    for (prefix, label) in &llm_group_labels {
        // Collect all coverage/precision/f1 values across all matching approaches and languages
        let mut cov_vals: Vec<f64> = Vec::new();
        let mut prec_vals: Vec<f64> = Vec::new();
        let mut f1_vals: Vec<f64> = Vec::new();

        for (approach, langs) in &approach_map {
            if approach.starts_with(prefix) {
                for la in langs.iter() {
                    cov_vals.extend_from_slice(&la.coverage.values);
                    prec_vals.extend_from_slice(&la.precision.values);
                    f1_vals.extend_from_slice(&la.f1.values);
                }
            }
        }

        if cov_vals.is_empty() {
            continue;
        }

        writeln!(s, "    \\addlinespace").unwrap();
        if let (Some(cov_st), Some(prec_st), Some(f1_st)) = (
            Stats::compute(&cov_vals),
            Stats::compute(&prec_vals),
            Stats::compute(&f1_vals),
        ) {
            write!(s, "    {} ", label).unwrap();
            write_coverage_row(&mut s, &cov_st, &prec_st, &f1_st);
        }
    }

    // --- iamfast per-language ---
    if let Some(langs) = approach_map.get("iamfast") {
        writeln!(s, "    \\addlinespace").unwrap();
        for la in langs.iter() {
            write!(s, "    iamfast ({}) ", short_lang_label(&la.language)).unwrap();
            write_coverage_row(&mut s, &la.coverage, &la.precision, &la.f1);
        }
    }

    writeln!(s, "    \\bottomrule").unwrap();
    writeln!(s, "  \\end{{tabular}}").unwrap();
    writeln!(s, "\\end{{table}}").unwrap();

    s
}

/// Write a single coverage table row: coverage, precision, F1 (mean ± std).
fn write_coverage_row(s: &mut String, cov: &Stats, prec: &Stats, f1: &Stats) {
    writeln!(
        s,
        "& {:.2} & $\\pm${:.2} & {:.2} & $\\pm${:.2} & {:.2} & $\\pm${:.2} \\\\",
        cov.mean, cov.std_dev,
        prec.mean, prec.std_dev,
        f1.mean, f1.std_dev,
    ).unwrap();
}


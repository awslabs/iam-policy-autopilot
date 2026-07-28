#!/usr/bin/env bash
#
# Generate the paper's LaTeX tables and plots from an aggregate_report.json
# produced by run_iac_benchmark.sh. This is entirely offline (no AWS calls
# except fetching the cached service catalogue for the coverage analysis).
#
# Usage:
#   ./paper-benchmarks/scripts/make_figures.sh <aggregate_report.json> [output-dir]
#
# Arguments:
#   aggregate_report.json   Path to the report from run_iac_benchmark.sh (required)
#   output-dir              Where to write .tex files [default: paper-benchmarks/paper/figures]
#
# Environment overrides:
#   RUNS_DIR   input runs directory used for coverage analysis
#              [default: integration-tests/projects]
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BENCH_DIR="$(dirname "$SCRIPT_DIR")"
REPO_ROOT="$(dirname "$BENCH_DIR")"

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <aggregate_report.json> [output-dir]" >&2
  exit 1
fi

AGG_REPORT="$1"
OUTPUT_DIR="${2:-$BENCH_DIR/paper/figures}"
RUNS_DIR="${RUNS_DIR:-$REPO_ROOT/integration-tests/projects}"

FIGURES="$BENCH_DIR/target/release/iac-paper-figures"
COVERAGE="$BENCH_DIR/target/release/iac-coverage-analyzer"

for f in "$FIGURES" "$COVERAGE"; do
  if [[ ! -x "$f" ]]; then
    echo "error: $f not found. Run scripts/build.sh first." >&2
    exit 1
  fi
done

if [[ ! -f "$AGG_REPORT" ]]; then
  echo "error: aggregate report not found: $AGG_REPORT" >&2
  exit 1
fi

REPORT_DIR="$(cd "$(dirname "$AGG_REPORT")" && pwd)"
COVERAGE_REPORT="$REPORT_DIR/coverage_report.json"

echo "==> Computing coverage / precision / F1 (offline)"
"$COVERAGE" \
  --aggregate-report "$AGG_REPORT" \
  --runs-dir "$RUNS_DIR" \
  --results-dir "$REPORT_DIR" || {
    echo "warning: coverage analyzer failed; generating figures without coverage table" >&2
    COVERAGE_REPORT=""
  }

mkdir -p "$OUTPUT_DIR"
echo "==> Generating LaTeX figures into $OUTPUT_DIR"
if [[ -n "$COVERAGE_REPORT" && -f "$COVERAGE_REPORT" ]]; then
  "$FIGURES" --input "$AGG_REPORT" --coverage-report "$COVERAGE_REPORT" --output-dir "$OUTPUT_DIR"
else
  "$FIGURES" --input "$AGG_REPORT" --output-dir "$OUTPUT_DIR"
fi

echo
echo "Done. LaTeX artefacts written to $OUTPUT_DIR"
echo "Preamble needs: \\usepackage{pgfplots}, \\usepgfplotslibrary{statistics}, \\usepackage{booktabs}, \\usepackage{siunitx}"

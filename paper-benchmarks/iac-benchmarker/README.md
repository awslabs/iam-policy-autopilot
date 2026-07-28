# iac-benchmarker

Minimum managed-policy finder and over-permission analyser for the IaC benchmark
runs. Part of the [`paper-benchmarks`](../README.md) workspace.

## Inputs

Each "run" is a small application directory containing a CDK stack (`cdk/`),
per-language data-plane scripts (`python/`, `go/`, `java/`, `typescript/`), and a
hand-minimized `minimal_policy.json` ground truth. The repository already ships
ten such runs at **`integration-tests/projects/run_001 … run_010`** — the
benchmark reuses those directly, so `--runs-dir integration-tests/projects` is
the normal invocation.

## Build

```sh
# From the paper-benchmarks workspace:
cargo build --release -p iac-benchmarker
```

This builds three binaries: `iac-benchmarker`, `iac-paper-figures`, and
`iac-coverage-analyzer`.

## Usage

### Batch mode (recommended)

Run the full benchmark pipeline across all `run_*` subdirectories and produce an
`aggregate_report.json`. Point `--autopilot-binary` at the release build of the
product binary (`../../target/release/iam-policy-autopilot` from here):

```sh
target/release/iac-benchmarker \
  --runs-dir ../../integration-tests/projects \
  --autopilot-binary ../../target/release/iam-policy-autopilot \
  --output-dir results/
```

Skip specific runs by directory name (space-separated or repeated flag):

```sh
target/release/iac-benchmarker \
  --runs-dir ../../integration-tests/projects \
  --skip run_001 run_005
```

### Single-run mode

```sh
target/release/iac-benchmarker \
  --run-dir ../../integration-tests/projects/run_001
```

## Key options

| Flag | Default | Description |
|---|---|---|
| `--runs-dir <DIR>` | — | Batch mode: directory containing `run_*` subdirs |
| `--run-dir <DIR>` | — | Single-run mode |
| `--skip <RUN_ID>...` | — | Skip one or more run directories by name (batch mode only) |
| `--output-dir <DIR>` | `benchmarker_results/<ts>/` | Where to write reports |
| `--cover-mode <MODE>` | `min-actions` | Set-cover strategy: `greedy`, `exact`, `min-actions` |
| `--autopilot-binary <PATH>` | `iam-policy-autopilot` | Pre-built IPA binary used to generate per-language policies |
| `--language <LANG>` | `java` | Language used for live validation |
| `--languages <LIST>` | `python,go,java,typescript` | Languages for autopilot overpermissioning |
| `--bedrock-model-id <ID>` | auto (region + account) | Bedrock model ID or inference profile ARN for LLM phase |
| `--bedrock-region <REGION>` | `--region` | AWS region for Bedrock calls |
| `--region <REGION>` | `us-east-1` | AWS region |
| `--skip-validation` | false | Skip live AWS execution (phases 4, 4b, 5c) |
| `--skip-llm` | false | Skip Bedrock LLM policy generation (phase 5c) |
| `--skip-deploy` | false | Assume CDK stack is already deployed |
| `--skip-destroy` | false | Leave CDK stack deployed after run |
| `--rebuild-index` | false | Force rebuild of policy index cache |
| `--cache-dir <DIR>` | `~/.iac-benchmarker/` | Directory for persistent cache files |

## Pipeline phases (per run)

```
3.5  CDK deploy
4    Validate managed-policy set via live execution
4b   Validate minimal_policy.json via live execution
5    Action-count comparison (managed vs minimal)
5b   Per-language autopilot overpermissioning
5c   Per-language LLM (Bedrock) overpermissioning + live validation
4.5  CDK destroy  ← after all live-execution phases
6    Write benchmark_managed_policies.json report
```

## Paper figures

Regenerate LaTeX/pgfplots figures from an existing aggregate report without
re-running benchmarks:

```sh
target/release/iac-paper-figures \
  --input results/aggregate_report.json \
  --output-dir paper/figures/
```

Add coverage/precision/F1 tables by first running the offline coverage analyzer
(no AWS calls beyond fetching the cached service catalogue) and passing its
output to the figure generator:

```sh
target/release/iac-coverage-analyzer \
  --aggregate-report results/aggregate/aggregate_report.json \
  --runs-dir ../../integration-tests/projects \
  --results-dir results/aggregate

target/release/iac-paper-figures \
  --input results/aggregate/aggregate_report.json \
  --coverage-report coverage_report.json \
  --output-dir paper/figures/
```

The LaTeX preamble must load `pgfplots` (with the `statistics` library),
`booktabs`, and `siunitx`; see the header comment in `src/bin/paper_figures.rs`.

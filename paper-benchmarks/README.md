# Artifact: IAM Policy Autopilot — Paper Benchmarks

## Notes on Cost and Safety

- Run all benchmarks in a **non-production** AWS account.
- The **synthetic IaC benchmark** deploys short-lived CDK stacks and creates
  temporary IAM roles. Stacks are destroyed automatically after each run. This
  incurs AWS charges.
- The **real-world benchmark** incurs costs when deploying the CDK stack
  (via `deploy_realworld_app.sh`) and running with `--app-url`. The static
  comparison mode (without `--app-url`) does not deploy anything. When deployed,
  the stack uses managed services — destroy it when finished.
- The **LLM experiments** (both benchmarks, when run without `--skip-llm`) incur
  AWS Bedrock invocation charges.

## Getting Started

### Artifact Description

This artifact accompanies the paper's experimental evaluation of IAM Policy
Autopilot (IPA). The artifact contains:

1. **IAM Policy Autopilot** — the tool itself with fixes applied between
   paper acceptance and camera-ready version, which improve some of IPA's results
   over what we reported in the paper.
2. **Synthetic IaC Benchmark** — 10 multi-language applications with ground-truth
   minimal IAM policies, used to measure over-permissioning of managed policies,
   LLM-generated policies, and IPA-generated policies
3. **Real-World Benchmark** — evaluation against a production-style multi-service
   AWS application (aws-genai-llm-chatbot)
4. **Reproduction scripts** — automated pipelines for all experiments

### Installation

**Load pre-built image**

```bash
docker load < ipa-paper-benchmarks-image.tar.gz
```

### Smoke Test

**Prerequisites:** AWS credentials configured in `~/.aws/` with IAM and
CloudFormation permissions in us-east-1. See REQUIREMENTS.md for details.

#### Synthetic IaC benchmark

Run a single synthetic benchmark (run_001) with LLM and iamfast experiments
disabled:

```bash
docker run --rm \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v ~/.aws:/root/.aws:ro \
  -e AWS_DEFAULT_REGION=us-east-1 \
  ipa-paper-benchmarks \
  -c "iac-benchmarker --run-dir /opt/integration-tests/projects/run_001 --skip-llm --skip-iamfast"
```

**Expected output** (last section):

```
======================================================================
  IAC BENCHMARKER RESULTS
======================================================================
  Run:                    run_001
  ...
  Set-cover coverage:     100.0%
  Validation:             PASSED (attempts: 1)
  Minimal policy valid:   PASSED
  ...
  Autopilot overpermissioning per language:
    Language           Actions     Minimal     Managed  vs Minimal  vs Managed   ...
    python                  14           3          23       4.67x       0.61x   ...
    go                       8           3          23       2.67x       0.35x   ...
    java                     8           3          23       2.67x       0.35x   ...
    typescript               8           3          23       2.67x       0.35x   ...
==========================================================================================
```

#### Real-world benchmark

Run IPA on a single handler from the real-world application (static comparison
against committed ground truth — no deployment required):

```bash
docker run --rm \
  -v ~/.aws:/root/.aws:ro \
  -e AWS_DEFAULT_REGION=us-east-1 \
  ipa-paper-benchmarks \
  -c "MODE=ipa ./scripts/run_realworld_eval.sh --only api-handler"
```

**Expected output** (last section):

```
=== Real-World App Evaluation Summary ===
Handlers: 1/1 succeeded
Avg CDK overpermission ratio: 1.35x
```

---

## Step-by-Step Reproduction Instructions

### Synthetic IaC Benchmark

#### Without LLM experiments (no Bedrock access required)

Runs all 10 benchmark applications with IPA policy generation in all 4
languages and managed-policy set-cover analysis:

```bash
docker run --rm \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v ~/.aws:/root/.aws:ro \
  -e AWS_DEFAULT_REGION=us-east-1 \
  -v $(pwd)/results:/opt/paper-benchmarks/results \
  ipa-paper-benchmarks \
  -c "./scripts/run_iac_benchmark.sh --skip-llm --skip-iamfast"
```

Results are written to `./results/` on the host.

#### With LLM experiments (requires Bedrock access)

A full run of all 10 applications including repeated LLM experiments takes
about 10 hours:

```bash
docker run --rm \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v ~/.aws:/root/.aws:ro \
  -e AWS_DEFAULT_REGION=us-east-1 \
  -v $(pwd)/results:/opt/paper-benchmarks/results \
  ipa-paper-benchmarks \
  -c "./scripts/run_iac_benchmark.sh"
```

#### Generating Figures

Generate the paper's LaTeX tables and plots from an aggregate report:

```bash
docker run --rm \
  -v $(pwd)/results:/opt/paper-benchmarks/results \
  ipa-paper-benchmarks \
  -c "./scripts/make_figures.sh results/aggregate/aggregate_report.json results/figures"
```

### Real-World Benchmark

The real-world benchmark evaluates IPA on `aws-samples/aws-genai-llm-chatbot`,
a production-style multi-LLM RAG chatbot. A full run including repeated LLM
experiments takes about 10 hours.

#### Without deployment (static comparison only)

Generates IPA policies and compares them against the committed CDK-deployed
ground truth. Does not require deploying the application:

```bash
docker run --rm \
  -v ~/.aws:/root/.aws:ro \
  -e AWS_DEFAULT_REGION=us-east-1 \
  -v $(pwd)/results:/opt/paper-benchmarks/real-world-apps/realworld_results \
  ipa-paper-benchmarks \
  -c "MODE=ipa ./scripts/run_realworld_eval.sh"
```

To run a single handler as a quick check:

```bash
docker run --rm \
  -v ~/.aws:/root/.aws:ro \
  -e AWS_DEFAULT_REGION=us-east-1 \
  ipa-paper-benchmarks \
  -c "MODE=ipa ./scripts/run_realworld_eval.sh --only api-handler"
```

#### With deployment and live validation

**Warning:** This deploys managed services 
that incur AWS charges. Destroy the stack when finished (step 3).

The CDK deploy must run **on the host** (not inside the container) because CDK's
asset bundling uses Docker volume mounts that require host filesystem access.
This requires Node.js 18-20 and AWS CDK on the host.

```bash
# 1. Deploy from the host (extracts app from image, builds, and deploys)
./deploy_realworld_app.sh
# Note the CloudFront URL from the output

# 2. Run the evaluation with live validation inside the container
docker run --rm \
  -v ~/.aws:/root/.aws:ro \
  -e AWS_DEFAULT_REGION=us-east-1 \
  -v $(pwd)/results:/opt/paper-benchmarks/real-world-apps/realworld_results \
  ipa-paper-benchmarks \
  -c "./scripts/run_realworld_eval.sh --app-url https://<distribution>.cloudfront.net"

# 3. Tear down when finished
cd realworld-deploy/genai-chatbot-cdk && npx cdk destroy
```

---

## Artifact Code Layout

```
paper-benchmarks/
├── Dockerfile                   # Container image definition
├── README.md                    # This file
├── REQUIREMENTS.md              # Hardware/software requirements
├── STATUS.md                    # Badge justification
├── LICENSE.md                   # License information
├── scripts/
│   ├── build.sh                 # Build all binaries (runs inside Dockerfile)
│   ├── run_iac_benchmark.sh     # Run synthetic IaC benchmark
│   ├── run_realworld_eval.sh    # Run real-world evaluation
│   ├── make_figures.sh          # Generate LaTeX tables/plots
│   └── prepare_realworld_app.sh # Extract pinned app tarball
├── iac-benchmarker/             # Synthetic benchmark: set-cover, over-permission analysis
├── realworld-evaluator/         # Real-world benchmark evaluator binary
├── real-world-apps/             # Real-world app handlers, scenarios, ground truth
├── runner/                      # Shared benchmark orchestration library
└── integration-tests/projects/run_001..run_010  # (in /opt/ in the image)
```

## Benchmark Methodology

### IaC benchmark (`iac-benchmarker/`)

Given a directory of "runs" (each a small application with a CDK stack,
per-language data-plane scripts, and a `minimal_policy.json` ground truth), the
benchmarker:

1. loads the required actions from `minimal_policy.json`;
2. finds the minimum set of **AWS managed policies** that covers them
   (set-cover); deploys the CDK stack and validates the managed-policy set with
   live execution;
3. generates a policy with **IPA** for each language and measures its
   over-permissioning vs. the minimal policy, and validates it live;
4. does the same for **LLM**-generated policies (via Bedrock, with several
   prompt/context strategies) and, optionally, **`iamfast`**;
5. writes a per-run report and an `aggregate_report.json`.

Then `iac-paper-figures` turns the aggregate report into LaTeX tables/plots, and
`iac-coverage-analyzer` computes coverage/precision/F1 offline. See
[`iac-benchmarker/README.md`](iac-benchmarker/README.md) for the full pipeline,
CLI flags, and figure outputs.

**Inputs:** the paper's 10 synthetic benchmarks live in
`/opt/integration-tests/projects/run_001 … run_010` inside the image.

### Real-world benchmark (`real-world-apps/`)

Evaluates IPA on `aws-samples/aws-genai-llm-chatbot`, a production-style
multi-LLM RAG chatbot. For each handler it compares IPA's generated policy against
the actual CDK-deployed IAM policy (the ground truth) and live-validates the IPA
policy by swapping it onto the handler's role and running targeted scenarios.

The full methodology, handler inventory, results, and deployment/build fixes are
documented in [`real-world-apps/EVALUATION.md`](real-world-apps/EVALUATION.md).

#### Ground-truth policies

The evaluator compares each generated policy against the handler's
**CDK-deployed** IAM policy. These developer-authored policies live under
`real-world-apps/genai-chatbot/handlers/<handler>/deployed_policy/`.

The evaluator only reads the **`Action`** fields of these documents for the
action-count comparison — it never reads resource ARNs. The versions committed
here have every statement's `Resource` set to `"*"`, which strips the
account- and deployment-specific resource names while preserving the exact
action sets that the numbers depend on.

To reproduce the ground truth against **your own** deployment (with real
resource ARNs), regenerate the files with
`real-world-apps/extract_deployed_policies.py`, which reads the inline and
attached policies straight off the deployed roles via the IAM API. This is
optional — the committed, wildcarded files are sufficient to reproduce the
paper's action counts.

## CLI Help

For a full list of flags and options:

```bash
docker run --rm ipa-paper-benchmarks -c "iac-benchmarker --help"
docker run --rm ipa-paper-benchmarks -c "realworld-evaluator --help"
```

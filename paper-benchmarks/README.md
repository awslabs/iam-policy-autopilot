# Paper benchmarks

This directory contains the benchmarks used in the paper's experimental
evaluation of `iam-policy-autopilot` (IPA). There are two independent
benchmarks:

| Benchmark | What it measures | Where |
|---|---|---|
| **IaC benchmark** (synthetic) | Over-permissioning of generated policies vs. a minimized ground truth, across 4 languages, plus comparison against AWS managed policies, LLM-generated policies, and `iamfast`. Produces the paper's LaTeX tables and plots. | [`iac-benchmarker/`](iac-benchmarker/) |
| **Real-world benchmark** | IPA vs. CDK-deployed ground truth on a real multi-service application (`aws-samples/aws-genai-llm-chatbot`), with live policy-swap validation. | [`real-world-apps/`](real-world-apps/) |

Both are gated behind an AWS account: they deploy infrastructure, create
temporary IAM roles, and run application code against live AWS APIs. **They
incur AWS charges**. Always tear the stacks down when finished.

## Layout

```
paper-benchmarks/
├── README.md                 # This file
├── Cargo.toml                # Self-contained Cargo workspace (separate from the repo root)
├── runner/                   # Benchmark orchestrator library (crate `iac_runner`, pkg `benchmark-runner`)
├── iac-benchmarker/          # IaC benchmark: set-cover, over-permission analysis, LaTeX figures
├── realworld-evaluator/      # Real-world benchmark evaluator binary
├── real-world-apps/          # Real-world app handlers, scenarios, and EVALUATION.md
└── scripts/                  # Convenience scripts to build and run the benchmarks
```

## Prerequisites

- **Rust** (stable) — `cargo`, `rustc`.
- **cmake** and a C/C++ compiler — required to build the `highs` LP solver used
  by the exact set-cover strategy.
- **x86_64 host** (or QEMU emulation) — the real-world app's Docker-based Lambda
  builds (langchain layer, Batch containers) target `linux/amd64`. On ARM hosts
  you may be able to use `DOCKER_DEFAULT_PLATFORM=linux/amd64` and `docker buildx`
  with QEMU, but this is untested.
- **AWS CLI credentials** for a non-production account, exported in the
  environment (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`).
  The credentials must be allowed to create/delete IAM roles, deploy
  CloudFormation/CDK stacks, and call the services each app uses.
- **Region: us-east-1** — the real-world app and all evaluation scripts assume
  `us-east-1`. Set `AWS_DEFAULT_REGION=us-east-1` before running `npx cdk deploy`
  (CDK reads `CDK_DEFAULT_REGION` which inherits from the AWS CLI default region).
- **Node.js + AWS CDK** (`npx cdk`) — for deploying the per-run CDK stacks.
- **Docker** — the real-world app's CDK deploy builds Lambda layers and Batch
  containers in Docker.
- **jq** — used by deploy scripts to parse CDK/CloudFormation outputs.
- Per-language toolchains for the scripts the benchmark executes: **Python 3**,
  **Go**, **Java + Maven**, **Node.js + ts-node**.
- **Python packages** for the real-world evaluation: `pip install boto3 pydantic "gql[aiohttp]"`.
- **Bedrock model access** — the LLM experiments and the `langchain-request-handler`
  live validation require an active Bedrock model. The model ID is configured in
  `real-world-apps/handler_scenarios.py`. AWS periodically retires models; if the
  configured model returns `ResourceNotFoundException` ("marked as Legacy") or
  `ValidationException` ("use an inference profile"), update the model ID:
  ```bash
  # List active models:
  aws bedrock list-foundation-models --region us-east-1 \
    --query "modelSummaries[?modelLifecycle.status=='ACTIVE'].modelId"
  # List inference profiles (required for newer models):
  aws bedrock list-inference-profiles --region us-east-1 \
    --query "inferenceProfileSummaries[].inferenceProfileId"
  ```
  Then update the model ID in `handler_scenarios.py` (search for the current value).
  Use a `us.*` inference profile ID for on-demand invocation.
- For the `iamfast` comparison (optional): the `iamfast` CLI on `PATH` (or pass
  `--skip-iamfast`).

## Quick start

```bash
# 1. Build the IPA binary (from the repo root) and the benchmark binaries.
#    On a source ZIP this first fetches the pinned submodule data (needs git).
./paper-benchmarks/scripts/build.sh

# 2. Run the synthetic IaC benchmark over the in-repo test projects.
#    Writes an aggregate_report.json and per-run logs.
./paper-benchmarks/scripts/run_iac_benchmark.sh

# 3. Generate the paper's LaTeX tables and plots from the aggregate report.
./paper-benchmarks/scripts/make_figures.sh <results-dir>/aggregate/aggregate_report.json

# 4. Real-world benchmark — extracts the pinned app tarball and deploys it to us-east-1.
#    If `npm run build` fails, see real-world-apps/EVALUATION.md § "Build fixes".
export AWS_DEFAULT_REGION=us-east-1
./paper-benchmarks/scripts/prepare_realworld_app.sh
cd paper-benchmarks/real-world-apps/genai-chatbot-cdk
npm install && npm run build
npx cdk bootstrap && npx cdk deploy   # note the CloudFront URL
cd -

# 5. Real-world benchmark — evaluate against the deployed stack.
#    MODE=all (default) runs IPA + LLM-bare + LLM-wildcards (5 trials each) + aggregation.
./paper-benchmarks/scripts/run_realworld_eval.sh --app-url https://<distribution>.cloudfront.net

# To run a single mode (e.g., IPA only):
MODE=ipa ./paper-benchmarks/scripts/run_realworld_eval.sh --app-url https://...
```

Each script accepts overrides via environment variables and forwards extra
arguments to the underlying binary; run a script with no changes to see its
defaults, or read the header comment in each script under
[`scripts/`](scripts/).

## The two benchmarks in detail

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
`integration-tests/projects/run_001 … run_010` at the repo root. These are the
applications used to produce Table 1 and Figure 3 in the paper.

### Real-world benchmark (`real-world-apps/`)

Evaluates IPA on `aws-samples/aws-genai-llm-chatbot`, a production-style
multi-LLM RAG chatbot with 18 Lambda/Batch handlers sharing a `genai_core`
library. For each handler it compares IPA's generated policy against the actual
CDK-deployed IAM policy (the ground truth) and live-validates the IPA policy by
swapping it onto the handler's role and running targeted scenarios.

The full methodology, handler inventory, results, and deployment/build fixes are
documented in [`real-world-apps/EVALUATION.md`](real-world-apps/EVALUATION.md).
The `genai-chatbot/` handler sources are copied from the upstream project (under
its MIT-0 license — see `real-world-apps/genai-chatbot/LICENSE`); the full app
you deploy is committed as a pinned tarball,
`real-world-apps/genai-chatbot-cdk-50b6c6e.tar.gz` (aws-genai-llm-chatbot at
commit `50b6c6e`), which you extract to `genai-chatbot-cdk/` — see EVALUATION.md.

#### Ground-truth policies (`deployed_policy/`)

The evaluator compares each generated policy against the handler's
**CDK-deployed** IAM policy. Those policies live under
`real-world-apps/genai-chatbot/handlers/<handler>/deployed_policy/` and are the
ground truth.

The evaluator only reads the **`Action`** fields of these documents for the
action-count comparison — it never reads resource ARNs. The versions committed
here therefore have every statement's `Resource` set to `"*"`, which strips the
account- and deployment-specific resource names from the paper's original
deployment while preserving the exact action sets that the numbers depend on.

To reproduce the ground truth against **your own** deployment (with real
resource ARNs), regenerate the files with
`real-world-apps/extract_deployed_policies.py`, which reads the inline and
attached policies straight off the deployed roles via the IAM API. This is
optional — the committed, wildcarded files are sufficient to reproduce the
paper's action counts.

## Cleaning up

- IaC benchmark: each run's CDK stack is destroyed automatically after
  validation (unless `--skip-destroy`). If a run is interrupted, destroy any
  leftover stack manually and delete stray `runner-role-*` IAM roles.
- Real-world benchmark: `cd paper-benchmarks/real-world-apps/genai-chatbot-cdk && npx cdk destroy`.

## Notes on cost and safety

- The benchmarks create **temporary IAM execution roles** scoped to the
  runner's own identity and delete them afterwards. Run them only in a
  **non-production** account.
- The real-world app deploys managed services. Do not leave the stack
  running.

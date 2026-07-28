# Real-World Application Evaluation

> **Quick path:** For prerequisites and the end-to-end command sequence, see
> [`../README.md`](../README.md) (steps 4–5). This document describes the
> methodology, handler inventory, results, and build fixes in detail.

## Goal

Evaluate IPA on a real multi-service application deployed to AWS. Deploy the app, extract the CDK-defined IAM policies as ground truth, run IPA against each handler, compare the generated policies, and validate them with live execution.

## Application: aws-samples/aws-genai-llm-chatbot

A production-style multi-LLM chatbot with RAG. 18 Python entry points (Lambda handlers + Batch jobs) sharing a `genai_core` library, using 14+ AWS services (Bedrock, DynamoDB, S3, Kendra, OpenSearch, Aurora, SNS, SQS, Cognito, Comprehend, SageMaker, Step Functions, Secrets Manager, SSM).

Repo: https://github.com/aws-samples/aws-genai-llm-chatbot
Version: `main` branch at commit `50b6c6e` (ahead of v5.0.0 tag, which has a deprecated Aurora PostgreSQL version).

### Versions used

| Component | Commit | Branch |
|---|---|---|
| aws-genai-llm-chatbot | `50b6c6e` | main |
| iam-policy-autopilot | `58b4aa3` | main |

## Deployment

The pinned upstream source is committed here as a tarball,
`genai-chatbot-cdk-50b6c6e.tar.gz` — a faithful copy of
`aws-samples/aws-genai-llm-chatbot` at commit
`50b6c6e08ea6e85cf8967c342b202d730062160e` (the codeload archive for that
commit; sha256 `9f2124969709870b79769bd31d416b5b1c0a3b090c13af2c69ea2822ab96fd39`).
The easiest way to unpack it and lay down the deploy config overlay is the prep
script, which extracts the tarball to `genai-chatbot-cdk/` (git-ignored) and
copies `config.json → bin/config.json` and `amplify/ → amplify/`:

```bash
./paper-benchmarks/scripts/prepare_realworld_app.sh
```

Equivalently, by hand:

```bash
cd paper-benchmarks/real-world-apps
tar xzf genai-chatbot-cdk-50b6c6e.tar.gz
mv aws-genai-llm-chatbot-50b6c6e genai-chatbot-cdk
cp genai-chatbot-cdk-config/config.json genai-chatbot-cdk/bin/config.json
cp -r genai-chatbot-cdk-config/amplify   genai-chatbot-cdk/amplify
```

(If you prefer to fetch upstream yourself instead of using the committed archive:
`git clone https://github.com/aws-samples/aws-genai-llm-chatbot.git genai-chatbot-cdk && git -C genai-chatbot-cdk checkout 50b6c6e`.)

Then apply the build fixes below and deploy with `npx cdk deploy`. The stack
must be deployed to **us-east-1** (`export AWS_DEFAULT_REGION=us-east-1`) — the
app's Bedrock config and the evaluation scripts assume this region.

### Build fixes required

The upstream project has several issues that must be resolved before building
and deploying. **All fixes for the pinned commit (`50b6c6e` / main branch) are
automated** by `prepare_realworld_app.sh` and the committed config overlay in
`genai-chatbot-cdk-config/`. No manual steps are needed beyond running the
script.

#### 1. `amplify codegen` requires scaffolding

The build script (`npm run build`) runs `amplify codegen && tsc`. The `amplify
codegen` step requires an initialized Amplify backend project directory that the
upstream repo does not include.

**Fix (automated):** `prepare_realworld_app.sh` copies
`genai-chatbot-cdk-config/amplify/` into the extracted app. This directory
contains the minimal project metadata, API config, GraphQL schema, and a
`local-env-info.json` so amplify resolves
the schema path correctly.

#### 2. tiktoken build failure (v5.0.0 only — does not apply)

On the v5.0.0 tag, `tiktoken` has no pre-built wheel for the Docker build
container. The pinned `main` branch commit uses `tiktoken>=0.5.0,<0.8.0` which
has wheels available.

#### 3. Aurora PostgreSQL version (v5.0.0 only — does not apply)

v5.0.0 uses Aurora PostgreSQL 15.7, which has been deprecated. The pinned
`main` branch commit uses 15.10.

### Configuration

Deployed with `bin/config.json` enabling:
- Bedrock (us-east-1)
- RAG with Aurora, OpenSearch, and Kendra engines
- Bedrock embedding models (no SageMaker — avoids GPU instance costs)

See `genai-chatbot-cdk-config/config.json` for the full configuration.

### Deployed stack outputs

After `npx cdk deploy` the stack prints its outputs. They are account- and
deployment-specific; the validation scripts discover them at runtime from the
CloudFormation stack, so you do not need to record them by hand. The relevant
outputs are:

| Resource | Value |
|---|---|
| UI (CloudFront) | `https://<distribution-id>.cloudfront.net` |
| Cognito User Pool | `<region>_<pool-id>` |
| GraphQL API (AppSync) | `https://<api-id>.appsync-api.<region>.amazonaws.com/graphql` |
| Stack ARN | `arn:aws:cloudformation:<region>:<ACCOUNT_NUMBER>:stack/GenAIChatBotStack/<stack-id>` |

Note: `cognitoFederation` is enabled with a dummy OIDC provider to deploy the
`add-user-to-group` handler. The Cognito user pool client needs
`ALLOW_ADMIN_USER_PASSWORD_AUTH` enabled for the test scripts —
`run_realworld_eval.sh` does this automatically before running the validation.

## Handler inventory

The chatbot has 18 handlers. Of these, 13 are deployed with our configuration, and 12 are included in the evaluation.

### Deployed and evaluated (12)

| Handler | Type | Lines | Files | Uses shared layer | How triggered |
|---|---|---|---|---|---|
| api-handler | Lambda | 11241 | 97 | yes | GraphQL queries/mutations |
| send-query-resolver | Lambda | 178 | 2 | no | `sendQuery` mutation |
| langchain-request-handler | Lambda | 12299 | 109 | yes | SQS (from send-query) |
| upload-handler | Lambda | 8691 | 70 | yes | `getUploadFileURL` query |
| create-aurora-workspace | Lambda | 7842 | 70 | yes | `createAuroraWorkspace` mutation |
| create-opensearch-workspace | Lambda | 7814 | 70 | yes | `createOpenSearchWorkspace` mutation |
| delete-document | Lambda | 9032 | 73 | yes | `deleteDocument` mutation |
| delete-workspace | Lambda | 8236 | 73 | yes | `deleteWorkspace` mutation |
| pg-setup | Lambda | 91 | 1 | no | Triggered during Aurora workspace creation |
| file-import-batch-job | Batch | 8720 | 71 | yes | Step Functions (from file upload) |
| web-crawler-batch-job | Batch | 7625 | 69 | yes | Step Functions (from `addWebsite` mutation) |
| add-user-to-group | Lambda | 186 | 1 | no | Cognito pre-signup/post-confirmation trigger |

### Deployed but not evaluated (1)

| Handler | Reason |
|---|---|
| bedrock-agents-handler | Requires an AgentCore runtime (containerized agent service). No AgentCore runtimes configured. |

### Not deployed (6)

| Handler | Reason |
|---|---|
| idefics-request-handler | Requires IDEFICS SageMaker model (`ml.g5.12xlarge`). Insufficient capacity in us-east-1 despite quota approval. |
| sagemaker-build-function | Only deploys with SageMaker models configured. Blocked by same capacity issue. |
| rss-ingestor | Excluded per original plan — schedule-triggered, hard to exercise on demand. |
| trigger-rss-ingestors | Excluded per original plan — schedule-triggered. |
| batch-crawl-rss-posts | Excluded per original plan — schedule-triggered. |

### Future work: expanding handler coverage

The two SageMaker handlers (`idefics-request-handler`, `sagemaker-build-function`) can be included in a follow-up evaluation if `ml.g5.12xlarge` capacity becomes available in the deployment region:

- `idefics-request-handler`: add `SupportedSageMakerModels.Idefics_9b` to `llms.sagemaker` (requires `ml.g5.12xlarge`).
- `sagemaker-build-function`: deploys when any SageMaker model is configured.

We attempted deployment with Idefics_9b but encountered insufficient capacity errors despite having the service quota approved. The `bedrock-agents-handler` would additionally require setting up an AgentCore runtime (containerized agent in ECR).

## Ground truth: deployed IAM policies

For each evaluated handler, we extracted the actual IAM policies from the
handler's deployed Lambda execution role after `cdk deploy`. CDK generates these
policies from the infrastructure code (`grantRead`, `grantWrite`,
`addToRolePolicy`, …), so they represent the developer-intended permissions and
serve as the ground-truth upper bound.

The extraction is done directly from the role via the IAM API (the same calls
`swap_validation.py` uses to snapshot and restore roles):

- **Inline policies** (`deployed_policy/inline_*.json`): from `iam:ListRolePolicies`
  + `iam:GetRolePolicy`. CDK-generated statements with specific actions and
  resource ARNs.
- **Attached managed policies** (`deployed_policy/attached_policies.json`): from
  `iam:ListAttachedRolePolicies`. AWS managed policies (e.g.,
  `AWSLambdaBasicExecutionRole`, `AWSLambdaVPCAccessExecutionRole`).

Both are included in the ground truth. Managed policies are expanded into
concrete actions using the service catalogue (same methodology as the synthetic
benchmark).

Policies are stored per-handler at `genai-chatbot/handlers/<handler>/deployed_policy/`.

> **Note on the committed ground truth:** only the `Action` fields of these
> documents are used for the CDK-vs-IPA action-count comparison — resource ARNs
> are never read. The committed files therefore have every statement's
> `Resource` set to `"*"`, which removes the account- and deployment-specific
> resource names from the paper's original deployment while preserving the exact
> action sets. To reproduce the ground truth against your own deployment (with
> real resource ARNs), regenerate the files with `extract_deployed_policies.py`,
> which reads them from the deployed roles via the IAM API.

## IPA evaluation

### Import tracing

Handlers that use the `genai_core` shared layer require import-aware file selection. The `realworld-evaluator` binary includes a Rust implementation of the import tracer. It performs regex-based BFS from the handler entry point through `genai_core` imports, passing only reachable shared layer files to IPA.

### Running IPA

The convenience script runs policy generation (IPA and/or LLM), live validation,
and aggregation end-to-end:

```bash
# All modes (IPA + LLM-bare + LLM-wildcards + aggregation):
./paper-benchmarks/scripts/run_realworld_eval.sh --app-url https://<distribution>.cloudfront.net

# IPA only:
MODE=ipa ./paper-benchmarks/scripts/run_realworld_eval.sh --app-url https://<distribution>.cloudfront.net
```

The script wraps the `realworld-evaluator` binary (policy generation + static
comparison) and `swap_validation.py` (live policy-swap tests). Results are written
to `realworld_results/<mode>/`. Omit `--app-url` to run phase 2 only (static
comparison against committed ground truth, no deployment needed).

Under the hood, the evaluator binary is invoked as:

```bash
paper-benchmarks/target/release/realworld-evaluator \
  --handlers-dir paper-benchmarks/real-world-apps/genai-chatbot/handlers \
  --shared-layer-dir paper-benchmarks/real-world-apps/genai-chatbot/shared_layer/genai_core \
  --autopilot-binary target/release/iam-policy-autopilot \
  --cache-dir paper-benchmarks/real-world-apps/.cache \
  --output-dir paper-benchmarks/real-world-apps/realworld_results/ipa \
  --mode ipa
```

IPA generated policies for all 13 handlers (12 evaluated + `bedrock-agents-handler` which we couldn't live-validate).

### IPA generation time

| Handler | Files | Lines | Generation time |
|---|---|---|---|
| add-user-to-group | 1 | 186 | 0.8s |
| api-handler | 97 | 11241 | 22.5s |
| bedrock-agents-handler | 74 | 8172 | 15.9s |
| create-aurora-workspace | 70 | 7842 | 15.1s |
| create-opensearch-workspace | 70 | 7814 | 15.0s |
| delete-document | 73 | 9032 | 17.6s |
| delete-workspace | 73 | 8236 | 16.0s |
| file-import-batch-job | 71 | 8720 | 16.9s |
| langchain-request-handler | 109 | 12299 | 23.9s |
| pg-setup | 1 | 91 | 0.4s |
| send-query-resolver | 2 | 178 | 0.8s |
| upload-handler | 70 | 8691 | 16.8s |
| web-crawler-batch-job | 69 | 7625 | 14.8s |

IPA generation time scales roughly linearly with file count. Small handlers (1-2 files) complete in under 1 second; the largest handler (langchain-request-handler, 109 files / 12K lines) takes ~24 seconds.

### Overpermissioning analysis and live validation

For each handler, we measured two things:
1. **Static analysis**: How many concrete IAM actions does the CDK policy grant vs. the IPA policy? (CDK/IPA ratio > 1 means CDK is more permissive.)
2. **Live validation**: Does the handler still work when its CDK policy is replaced with the IPA policy?

| Handler | CDK actions | IPA actions | CDK/IPA ratio | Live validation |
|---|---|---|---|---|
| add-user-to-group | 6 | 3 | 2.00x | pass |
| api-handler | 104 | 83 | 1.25x | partial (4/16 pass) |
| create-aurora-workspace | 22 | 60 | 0.37x | pass |
| create-opensearch-workspace | 23 | 60 | 0.38x | pass |
| delete-document | 79 | 60 | 1.32x | pass |
| delete-workspace | 79 | 60 | 1.32x | pass |
| file-import-batch-job | 84 | 62 | 1.35x | pass |
| langchain-request-handler | 339 | 70 | 4.84x | pass |
| pg-setup | 11 | 2 | 5.50x | pass |
| send-query-resolver | 18 | 5 | 3.60x | pass |
| upload-handler | 88 | 60 | 1.47x | pass |
| web-crawler-batch-job | 84 | 61 | 1.38x | pass |
| **Aggregate** | **937** | **586** | **1.60x** | **11/12 pass** |

**CDK policies are on average 1.6x more permissive than IPA-generated policies** (aggregate factor: sum of CDK actions / sum of IPA actions across all handlers). For `langchain-request-handler` (4.84x) and `pg-setup` (5.50x), the overpermissioning is significant.

Note: `create-aurora-workspace` and `create-opensearch-workspace` show IPA generating more actions than CDK (ratio < 1). This is because IPA analyzes the full shared layer reachable from these handlers, generating permissions for code paths that these specific handlers don't exercise at runtime.

## Live validation methodology

### Policy-swap procedure

For each handler:
1. Save the original CDK-deployed inline policies and attached managed policies
2. Remove all inline policies and detach all managed policies from the execution role
3. Put only the IPA-generated policy as the sole inline policy (no managed policies)
4. Run targeted scenarios through the GraphQL API
5. Restore the original inline and managed policies

Only one handler's role is swapped at a time. All other handlers retain their original CDK policies during the test. This isolates the effect: if a scenario fails, the failure is attributable to the swapped handler's IPA policy.

### Scenario-to-handler mapping

Each handler is validated by running the scenario groups that exercise it. The mapping is derived from the application's CDK construct graph and event flow (which AppSync resolvers route to which Lambdas, which Step Functions invoke which Batch jobs, etc.).

| Handler | Scenario groups run | What's tested |
|---|---|---|
| api-handler | api-handler, embedding, kendra-workspace, aurora-workspace, opensearch-workspace, upload-handler, delete-document, web-crawler-batch-job | 7 GraphQL list operations + Bedrock embedding + all workspace/document/upload/crawl operations |
| send-query-resolver | send-query-resolver | Send chat message (SNS publish) |
| langchain-request-handler | langchain-request-handler | End-to-end chat: send query, wait, verify session |
| upload-handler | upload-handler | Presigned S3 upload URL generation |
| create-aurora-workspace | aurora-workspace | Aurora workspace create + delete lifecycle |
| pg-setup | aurora-workspace | Aurora workspace lifecycle (pg-setup triggered internally) |
| create-opensearch-workspace | opensearch-workspace | OpenSearch workspace create + delete lifecycle |
| delete-document | delete-document | Delete document from Kendra, Aurora, and OpenSearch backends |
| delete-workspace | kendra-workspace, aurora-workspace, opensearch-workspace | Workspace create + delete lifecycle for all three engine types |
| file-import-batch-job | file-import-batch-job | File upload → Step Function → Batch import pipeline |
| web-crawler-batch-job | web-crawler-batch-job | Add website URL → Step Function → Batch crawl |
| add-user-to-group | add-user-to-group | Cognito post-confirmation trigger (direct Lambda invocation) |

Note on api-handler coverage: The api-handler serves as the GraphQL resolver for all queries and mutations. When its role is swapped, we run all scenario groups that route through it — including workspace lifecycle, document operations, file upload, and web crawling — in addition to the direct list/embedding operations. This provides comprehensive coverage of api-handler's code paths under the IPA-generated policy.

### Analysis of api-handler failures

The 12 api-handler failures are caused by two missing permissions, both hidden behind `aws_lambda_powertools.utilities.parameters` abstractions:

1. `ssm:GetParameter` — called via `parameters.get_parameter()` to fetch the shared application config from SSM Parameter Store. This affects every route that calls `genai_core.parameters.get_config()` (11 of 12 failing scenarios).

2. `secretsmanager:GetSecretValue` — called via `parameters.get_secret()` to fetch API keys (OpenAI, Azure OpenAI) from Secrets Manager. This affects `list_models`, which enumerates models from external providers.

The call chains are:

```
# SSM path (11 failing scenarios)
routes/*.py → genai_core.parameters.get_config()
  → aws_lambda_powertools.utilities.parameters.get_parameter()
    → boto3.client("ssm").get_parameter()  ← AccessDenied

# Secrets Manager path (list_models)
routes/models.py → genai_core.models.list_models()
  → genai_core.clients.get_openai_client()
    → genai_core.parameters.get_external_api_key("OPENAI_API_KEY")
      → aws_lambda_powertools.utilities.parameters.get_secret()
        → boto3.client("secretsmanager").get_secret_value()  ← AccessDenied
```

IPA detected both SSM and Secrets Manager as services used by the handler (`ssm:ListDocuments` and `secretsmanager:ListSecrets` appear in the generated policy) but missed the specific read operations because the calls go through `aws_lambda_powertools.utilities.parameters` — an official AWS library that wraps boto3 calls. IPA does not trace through this abstraction layer.

The 4 passing api-handler scenarios (`list_workspaces`, `list_sessions`, `list_roles`, `list_applications`) use DynamoDB directly without hitting either the SSM or Secrets Manager paths.

### Verification: IPA policy + missing permissions

To confirm that the two `aws_lambda_powertools` abstractions are the only gap, we augmented the IPA-generated policy with `ssm:GetParameter` and `secretsmanager:GetSecretValue`, then re-ran all 19 api-handler scenarios (7 list operations + embedding + 3 workspace lifecycles + upload + 3 document deletions + web crawl). All 19 passed, confirming that the IPA policy is otherwise complete and the only missing permissions are the two `aws_lambda_powertools`-wrapped SDK calls.

## LLM-based policy generation experiments

In addition to IPA, we evaluated LLM-generated policies using Claude Sonnet 4.6 (via Bedrock) with the same prompts used in the synthetic benchmarks. Two resource prompt strategies were tested, each with 5 independent trials per handler to measure variance:

1. **Bare** — prompt: "Generate an identity-based AWS IAM Policy which allows me to execute this application." No resource instruction.
2. **Wildcards** — prompt adds: "Fill in all placeholder variables; if you don't know what to put, use the wildcard *."

All strategies receive the same concatenated handler + shared layer source code as IPA.

### LLM bare results (5 trials per handler)

| Handler | CDK actions | LLM actions (median) | CDK/LLM ratio | Live validation (trials pass) |
|---|---|---|---|---|
| add-user-to-group | 6 | 6 | 1.00x | 2/5 |
| api-handler | 104 | 34 | 3.06x | 0/5 |
| create-aurora-workspace | 22 | 12 | 1.83x | 1/5 |
| create-opensearch-workspace | 23 | 9 | 2.56x | 3/5 |
| delete-document | 79 | 21 | 3.76x | 1/5 |
| delete-workspace | 79 | 19 | 4.16x | 1/5 |
| file-import-batch-job | 84 | 32 | 2.62x | 0/5 |
| langchain-request-handler | 339 | 212 | 1.60x | 3/5 |
| pg-setup | 11 | 6 | 1.83x | 4/5 |
| send-query-resolver | 18 | 9 | 2.00x | 0/5 |
| upload-handler | 88 | 43 | 2.05x | 5/5 |
| web-crawler-batch-job | 84 | 17 | 4.94x | 0/5 |
| **Aggregate** | **937** | **420** | **2.23x** | **1/12 handlers: 5/5** |

The bare strategy performs poorly on live validation — only `upload-handler` passes all 5 trials. Without resource instructions, the LLM generates specific resource ARNs that rarely match the deployed infrastructure. Many policies are also malformed (high Access Analyzer error counts).

### LLM wildcards results (5 trials per handler)

| Handler | CDK actions | LLM actions (median) | CDK/LLM ratio | Live validation (trials pass) |
|---|---|---|---|---|
| add-user-to-group | 6 | 3 | 2.00x | 5/5 |
| api-handler | 104 | 40 | 2.60x | 1/5 |
| create-aurora-workspace | 22 | 186 | 0.12x | 5/5 |
| create-opensearch-workspace | 23 | 34 | 0.68x | 5/5 |
| delete-document | 79 | 29 | 2.72x | 5/5 |
| delete-workspace | 79 | 33 | 2.39x | 5/5 |
| file-import-batch-job | 84 | 35 | 2.40x | 5/5 |
| langchain-request-handler | 339 | 211 | 1.61x | 5/5 |
| pg-setup | 11 | 8 | 1.38x | 5/5 |
| send-query-resolver | 18 | 11 | 1.64x | 5/5 |
| upload-handler | 88 | 36 | 2.44x | 5/5 |
| web-crawler-batch-job | 84 | 188 | 0.45x | 5/5 |
| **Aggregate** | **937** | **814** | **1.15x** | **11/12 handlers: 5/5** |

The api-handler failures (4/5 trials) are caused by **resource ARN mismatches**: the LLM guessed resource patterns like `arn:aws:ssm:*:*:parameter/*ConfigParameter*` that don't match the CDK-generated names (e.g., `CFN-SharedConfig358B4A20-6LBjNlmyRxpN`). The LLM correctly identified the required actions (including `ssm:GetParameter` that IPA missed) but the resource constraints were too narrow. One trial happened to generate patterns broad enough to match.

### Failure modes across experiments

| Failure mode | IPA | LLM bare | LLM wildcards |
|---|---|---|---|
| Missing action (aws_lambda_powertools abstraction) | api-handler: `ssm:GetParameter`, `secretsmanager:GetSecretValue` | — | — |
| Resource ARN mismatch | — | Most handlers: LLM-guessed ARNs don't match deployed resources | api-handler: 4/5 trials fail |
| Malformed policy (syntax errors) | — | Many handlers: high AA error counts | — |

Key observations:
- **IPA** misses SDK calls hidden behind `aws_lambda_powertools` but generates correct resource-scoped policies for everything else. Deterministic — no variance across runs.
- **LLM bare** generates the most specific policies (highest CDK/LLM ratio at 2.54x) but fails live validation for most handlers because the guessed resource ARNs are wrong and policies are often malformed.
- **LLM wildcards** correctly identifies all actions (including the ones IPA misses) but generates resource ARN patterns that don't match CDK-generated names. Shows variance: api-handler passes in 1/5 trials when the LLM happens to generate broad enough patterns.

### Summary across all approaches

| Approach | Agg. CDK/generated factor | Handlers with 5/5 live validation | Failure mode |
|---|---|---|---|
| IPA | 1.60x | 11/12 | Missing `aws_lambda_powertools` actions |
| LLM bare | 2.23x | 1/12 | Resource ARN mismatches + malformed policies |
| LLM wildcards | 1.15x | 11/12 | Resource ARN mismatches (api-handler only) |

## File layout

```
paper-benchmarks/real-world-apps/
├── EVALUATION.md                       # This document
├── handler_scenarios.py                # Scenario tests for live validation
├── swap_validation.py                  # Policy-swap validation orchestrator
├── extract_deployed_policies.py        # Regenerate deployed_policy/ ground truth from a live deployment
├── aggregate_results.py                # Aggregate results into the paper's summary table
├── genai-chatbot/                      # Handler source code + deployed policies (from aws-samples/aws-genai-llm-chatbot, MIT-0)
│   ├── LICENSE                         # Upstream MIT-0 license
│   ├── shared_layer/genai_core/        # Shared Python library
│   └── handlers/                       # Per-handler directories
│       └── <handler>/
│           ├── handler/                # Source code
│           ├── deployed_policy/        # CDK ground-truth IAM policies (resources wildcarded; see note above)
│           └── metadata.json           # Description + services
├── genai-chatbot-cdk-config/           # Amplify scaffolding + deployment config
│   ├── amplify/                        # Minimal Amplify project files
│   └── config.json                     # CDK deployment configuration
├── genai-chatbot-cdk/                  # (git-ignored) full upstream clone you create for deployment
└── realworld_results/                  # (git-ignored) evaluation outputs
    ├── baseline_scenarios.json         # Baseline test results (CDK policies)
    ├── ipa/                            # IPA evaluation + swap validation
    ├── llm-bare/                       # LLM bare evaluation + swap validation (5 trials)
    └── llm-wildcards/                  # LLM wildcards evaluation + swap validation (5 trials)
```

Import tracing and policy comparison logic lives in the `realworld-evaluator` Rust binary
(`paper-benchmarks/realworld-evaluator/`), which reuses `iac-benchmarker` for IAM glob
matching, service catalogue expansion, and managed policy indexing.

## Cleanup

Delete the CloudFormation stack to stop ongoing charges:

```bash
cd paper-benchmarks/real-world-apps/genai-chatbot-cdk
npx cdk destroy
```

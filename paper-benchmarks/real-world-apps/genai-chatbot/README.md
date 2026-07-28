# GenAI LLM Chatbot - Real-World Application Benchmark

Source: [aws-samples/aws-genai-llm-chatbot](https://github.com/aws-samples/aws-genai-llm-chatbot)

## Overview

This benchmark evaluates IAM policy inference on a real-world multi-service
application rather than isolated scripts. The application is a multi-LLM chatbot
with RAG capabilities, deployed as multiple Lambda functions sharing a common
Python SDK layer (`genai_core`).

## Key Differences from Script Benchmarks

1. **Cross-file dependency resolution**: Handlers import from a shared `genai_core`
   library. The tool must trace imports across files to discover all AWS SDK calls.
2. **Conditional service usage**: Some code paths only execute depending on
   configuration (e.g., Kendra, Aurora, OpenSearch engines).
3. **Multiple distinct roles**: Each Lambda needs its own least-privilege policy,
   even though many share the same library.
4. **Indirect SDK usage**: LangChain wraps Bedrock calls; the tool must recognize
   transitive SDK dependencies.

## Ground Truth

Policies are derived from the CDK infrastructure code (`grantRead`, `grantWrite`,
`addToRolePolicy` calls). These represent developer-intended permissions and serve
as an upper bound — the CDK may grant more than strictly needed.

## Structure

Each subdirectory under `handlers/` contains:
- `handler/` — The Lambda handler code and its local dependencies
- `shared_layer/` — Symlink or copy of the shared `genai_core` library
- `cdk_policy.json` — Ground truth IAM policy extracted from CDK
- `metadata.json` — Handler metadata (services used, description, LOC)

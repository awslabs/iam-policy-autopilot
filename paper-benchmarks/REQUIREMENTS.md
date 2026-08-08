# Requirements

The Quick Start path described in the README.md can be run on any architecture
or operating system. Software dependencies are minimal (and will be obvious)
from the description.

The following describes the requirements for replicating the results from the
paper, which are stricter.

## Architecture

- **x86_64 (amd64)** — the Docker image is built for linux/amd64.

## Software

- **Docker** — tested with version 25.0.14. The daemon must be running.
  Required to load and run the artifact image.
- **Node.js 18-20 + AWS CDK** (for real-world benchmark deployment only) —
  the real-world app deployment runs on the host because CDK's Docker-based
  asset bundling requires host filesystem access. Install with:
  `npm install -g aws-cdk`. Not needed for the synthetic benchmark or the
  static comparison mode of the real-world benchmark.

## Hardware

- **CPU:** any modern x86_64 processor
- **RAM:** ~8 GB minimum
- **Storage:** ~3 GB (800 MB compressed artifact + 2.15 GB uncompressed Docker image
  after loading)

## Network

Network access is required to call AWS APIs and for IAM Policy Autopilot to
communicate with the service reference endpoing at runtime.

## AWS Account

An AWS account is required to run the benchmarks (they deploy infrastructure and
create temporary IAM roles). The account must have:

- **Region:** us-east-1
- **Permissions:**:
  - **IAM permissions:** CreateRole, DeleteRole, PutRolePolicy, DeleteRolePolicy,
    ListPolicies, GetPolicy, GetPolicyVersion, AttachRolePolicy, DetachRolePolicy
  - **CloudFormation/CDK permissions:** full stack create/update/delete
  - **Service permissions:** varies by benchmark run (S3, DynamoDB, SQS, Redshift,
  Lambda, etc.)
  - **Bedrock access** (for LLM experiments only): invoke model permission on at
    least one active foundation model in us-east-1

Credentials are provided by mounting `~/.aws:/root/.aws:ro` into the container
(see `README.md`).

## Cost Warning

The benchmarks deploy real AWS infrastructure and incur charges.

## Operating System

The artifact runs inside a Docker container (Debian Bookworm). The host OS can
be any system that runs Docker: Linux, macOS, or Windows (via WSL2/Docker Desktop).
We only tested on Linux, however.
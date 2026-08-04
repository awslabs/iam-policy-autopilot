# Status

## Badges Applied For

**Available** — **Functional** — **Reusable**

## Justification

### Available

The artifact is publicly available under the Apache-2.0 open source license
(the real-world application component is under MIT-0). The source code is
hosted on GitHub at https://github.com/awslabs/iam-policy-autopilot and the
Docker image can be rebuilt from source.

### Functional

The artifact includes:
- The complete IAM Policy Autopilot tool (source + pre-built binary)
- All 10 synthetic IaC benchmark projects used to produce Table 1 and Figure 3
- The real-world application benchmark (pinned at commit 50b6c6e) for Table 2
- Scripts to reproduce all experimental results from the paper
- A Docker image with all dependencies pre-installed

The artifact can reproduce the paper's key claims:
- Over-permissioning ratios of managed policies vs. minimal policies (Table 1)
- Per-language IPA policy generation accuracy (Figure 3)
- Real-world application policy comparison (Table 2)
- Live validation of generated policies against deployed infrastructure

### Reusable

The artifact is designed for reuse beyond the paper's evaluation:
- IAM Policy Autopilot is an actively maintained open source tool usable on
  arbitrary AWS applications
- The benchmarking framework accepts new run directories (applications) without
  code changes
- The Docker image provides a complete, reproducible environment for IAM policy
  research
- All scripts accept configuration via environment variables and CLI flags
- The code is well-documented with README files for each component

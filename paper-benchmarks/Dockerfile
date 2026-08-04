# Paper-benchmarks artifact image
#
# Build from the REPO ROOT (not paper-benchmarks/). The Go and AWS CLI installs
# below fetch amd64 tarballs, so the platform must be pinned — otherwise a build
# on an arm64 host yields a broken mixed-arch image instead of failing:
#   docker build --platform linux/amd64 -f paper-benchmarks/Dockerfile -t ipa-paper-benchmarks .
#
# Prefer scripts/package_artifact.sh, which pins the platform and verifies it.
#
# Run:
#   docker run --rm -it \
#     -v /var/run/docker.sock:/var/run/docker.sock \
#     -v ~/.aws:/root/.aws:ro \
#     -e AWS_DEFAULT_REGION=us-east-1 \
#     ipa-paper-benchmarks
#
# The image contains all binaries, benchmark data, and toolchains needed to
# reproduce the paper's evaluation. AWS credentials must be provided at runtime.

# =============================================================================
# Stage 1: Build Rust binaries
# =============================================================================
FROM rust:1.97-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake g++ libssl-dev pkg-config git python3 libclang-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .

# build.sh handles submodule setup (fetches pinned commits over HTTPS even from
# a source ZIP with no .git), then compiles IPA + benchmark binaries.
RUN SKIP_DEP_CHECK=1 ./paper-benchmarks/scripts/build.sh

# =============================================================================
# Stage 2: Runtime image with all toolchains
# =============================================================================
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    curl ca-certificates git jq unzip xz-utils \
    python3 python3-pip python3-venv \
    default-jdk maven \
    docker.io \
    && rm -rf /var/lib/apt/lists/*

# Go (specific version for reproducibility)
RUN curl -fsSL https://go.dev/dl/go1.25.12.linux-amd64.tar.gz | tar -C /usr/local -xz
ENV PATH="/usr/local/go/bin:${PATH}"

# Node.js 20 LTS
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
    && apt-get install -y nodejs \
    && npm install -g ts-node typescript aws-cdk \
    && rm -rf /var/lib/apt/lists/*

# AWS CLI v2
RUN curl -fsSL "https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip" -o /tmp/awscli.zip \
    && unzip -q /tmp/awscli.zip -d /tmp \
    && /tmp/aws/install \
    && rm -rf /tmp/awscli.zip /tmp/aws

# Python packages for real-world evaluation
RUN pip3 install --break-system-packages boto3 pydantic "gql[aiohttp]"

# Copy built Rust binaries
COPY --from=builder /src/target/release/iam-policy-autopilot /usr/local/bin/
COPY --from=builder /src/paper-benchmarks/target/release/iac-benchmarker /usr/local/bin/
COPY --from=builder /src/paper-benchmarks/target/release/iac-paper-figures /usr/local/bin/
COPY --from=builder /src/paper-benchmarks/target/release/iac-coverage-analyzer /usr/local/bin/
COPY --from=builder /src/paper-benchmarks/target/release/realworld-evaluator /usr/local/bin/

# Copy benchmark data and scripts
COPY integration-tests/projects/ /opt/integration-tests/projects/
COPY paper-benchmarks/scripts/ /opt/paper-benchmarks/scripts/
COPY paper-benchmarks/real-world-apps/ /opt/paper-benchmarks/real-world-apps/
COPY paper-benchmarks/iac-benchmarker/scenarios/ /opt/paper-benchmarks/iac-benchmarker/scenarios/
COPY paper-benchmarks/README.md /opt/paper-benchmarks/

# Extract pinned real-world app and apply config overlay (mirrors prepare_realworld_app.sh)
RUN cd /opt/paper-benchmarks/real-world-apps \
    && tar -xzf genai-chatbot-cdk-50b6c6e.tar.gz \
    && mv aws-genai-llm-chatbot-50b6c6e genai-chatbot-cdk \
    && cp genai-chatbot-cdk-config/config.json genai-chatbot-cdk/bin/config.json \
    && cp -r genai-chatbot-cdk-config/amplify genai-chatbot-cdk/amplify \
    && rm genai-chatbot-cdk-50b6c6e.tar.gz

# Symlink binaries where scripts expect them
RUN mkdir -p /opt/paper-benchmarks/target/release /opt/target/release \
    && ln -s /usr/local/bin/realworld-evaluator /opt/paper-benchmarks/target/release/realworld-evaluator \
    && ln -s /usr/local/bin/iac-benchmarker /opt/paper-benchmarks/target/release/iac-benchmarker \
    && ln -s /usr/local/bin/iac-paper-figures /opt/paper-benchmarks/target/release/iac-paper-figures \
    && ln -s /usr/local/bin/iac-coverage-analyzer /opt/paper-benchmarks/target/release/iac-coverage-analyzer \
    && ln -s /usr/local/bin/iam-policy-autopilot /opt/target/release/iam-policy-autopilot

ENV AWS_DEFAULT_REGION=us-east-1
WORKDIR /opt/paper-benchmarks

ENTRYPOINT ["/bin/bash"]

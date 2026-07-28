"""Configuration loader for the document processing pipeline.

Reads settings from environment variables, with optional fallback to
AWS SSM Parameter Store for secrets and deployment-specific values.
"""

import os
import logging
from dataclasses import dataclass, field
from typing import Optional

import boto3
from botocore.exceptions import ClientError

logger = logging.getLogger(__name__)


@dataclass
class PipelineConfig:
    """All runtime configuration for the document processing pipeline."""

    # S3
    document_bucket: str
    output_prefix: str = "processed/"

    # SQS
    document_queue_url: str = ""
    max_messages: int = 10
    visibility_timeout: int = 300
    wait_time_seconds: int = 20

    # DynamoDB
    metadata_table: str = "document-metadata"

    # SNS
    results_topic_arn: str = ""

    # Processing
    max_retries: int = 3
    log_level: str = "INFO"

    # SSM path prefix for secrets (optional)
    ssm_prefix: str = "/document-pipeline"

    # Runtime-resolved secrets (populated by load_secrets)
    encryption_key_id: Optional[str] = field(default=None, repr=False)


def load_config() -> PipelineConfig:
    """Load configuration from environment variables."""
    cfg = PipelineConfig(
        document_bucket=_require_env("DOCUMENT_BUCKET"),
        output_prefix=os.environ.get("OUTPUT_PREFIX", "processed/"),
        document_queue_url=_require_env("DOCUMENT_QUEUE_URL"),
        max_messages=int(os.environ.get("MAX_MESSAGES", "10")),
        visibility_timeout=int(os.environ.get("VISIBILITY_TIMEOUT", "300")),
        wait_time_seconds=int(os.environ.get("WAIT_TIME_SECONDS", "20")),
        metadata_table=os.environ.get("METADATA_TABLE", "document-metadata"),
        results_topic_arn=_require_env("RESULTS_TOPIC_ARN"),
        max_retries=int(os.environ.get("MAX_RETRIES", "3")),
        log_level=os.environ.get("LOG_LEVEL", "INFO"),
        ssm_prefix=os.environ.get("SSM_PREFIX", "/document-pipeline"),
    )
    return cfg


def load_secrets(cfg: PipelineConfig) -> PipelineConfig:
    """Optionally enrich config with secrets from SSM Parameter Store."""
    ssm = boto3.client("ssm")
    key_param = f"{cfg.ssm_prefix}/encryption-key-id"
    try:
        resp = ssm.get_parameter(Name=key_param, WithDecryption=True)
        cfg.encryption_key_id = resp["Parameter"]["Value"]
        logger.info("Loaded encryption key ID from SSM: %s", key_param)
    except ClientError as exc:
        code = exc.response["Error"]["Code"]
        if code == "ParameterNotFound":
            logger.debug("SSM parameter %s not found — using default encryption", key_param)
        else:
            logger.warning("Could not load SSM parameter %s: %s", key_param, exc)
    return cfg


def _require_env(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        raise EnvironmentError(f"Required environment variable '{name}' is not set.")
    return value

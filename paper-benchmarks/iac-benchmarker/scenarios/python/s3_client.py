"""S3 client helpers for the document processing pipeline.

Wraps boto3 S3 operations with retry logic, structured logging,
and optional server-side encryption using a KMS key.
"""

import io
import logging
from pathlib import Path
from typing import Optional, Union

import boto3
from botocore.exceptions import ClientError

logger = logging.getLogger(__name__)


class S3Client:
    """High-level S3 operations for the document pipeline."""

    def __init__(self, bucket: str, kms_key_id: Optional[str] = None):
        self._bucket = bucket
        self._kms_key_id = kms_key_id
        self._s3 = boto3.client("s3")

    # ------------------------------------------------------------------
    # Upload
    # ------------------------------------------------------------------

    def upload_file(self, local_path: Union[str, Path], s3_key: str) -> str:
        """Upload a local file to S3 and return the object URL."""
        local_path = Path(local_path)
        extra_args = self._sse_args()
        logger.info("Uploading %s → s3://%s/%s", local_path, self._bucket, s3_key)
        self._s3.upload_file(
            str(local_path),
            self._bucket,
            s3_key,
            ExtraArgs=extra_args,
        )
        return f"s3://{self._bucket}/{s3_key}"

    def upload_bytes(self, data: bytes, s3_key: str, content_type: str = "application/octet-stream") -> str:
        """Upload raw bytes to S3."""
        extra_args = {**self._sse_args(), "ContentType": content_type}
        logger.debug("Uploading %d bytes → s3://%s/%s", len(data), self._bucket, s3_key)
        self._s3.put_object(
            Bucket=self._bucket,
            Key=s3_key,
            Body=data,
            **extra_args,
        )
        return f"s3://{self._bucket}/{s3_key}"

    # ------------------------------------------------------------------
    # Download
    # ------------------------------------------------------------------

    def download_bytes(self, s3_key: str) -> bytes:
        """Download an S3 object and return its content as bytes."""
        logger.debug("Downloading s3://%s/%s", self._bucket, s3_key)
        buf = io.BytesIO()
        self._s3.download_fileobj(self._bucket, s3_key, buf)
        return buf.getvalue()

    def download_file(self, s3_key: str, local_path: Union[str, Path]) -> None:
        """Download an S3 object to a local file."""
        local_path = Path(local_path)
        local_path.parent.mkdir(parents=True, exist_ok=True)
        logger.info("Downloading s3://%s/%s → %s", self._bucket, s3_key, local_path)
        self._s3.download_file(self._bucket, s3_key, str(local_path))

    # ------------------------------------------------------------------
    # Metadata / listing
    # ------------------------------------------------------------------

    def head_object(self, s3_key: str) -> Optional[dict]:
        """Return object metadata, or None if the object does not exist."""
        try:
            return self._s3.head_object(Bucket=self._bucket, Key=s3_key)
        except ClientError as exc:
            if exc.response["Error"]["Code"] in ("404", "NoSuchKey"):
                return None
            raise

    def list_objects(self, prefix: str) -> list[dict]:
        """List all objects under *prefix*, handling pagination."""
        paginator = self._s3.get_paginator("list_objects_v2")
        results = []
        for page in paginator.paginate(Bucket=self._bucket, Prefix=prefix):
            results.extend(page.get("Contents", []))
        return results

    def delete_object(self, s3_key: str) -> None:
        """Delete a single S3 object."""
        logger.debug("Deleting s3://%s/%s", self._bucket, s3_key)
        self._s3.delete_object(Bucket=self._bucket, Key=s3_key)

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    def _sse_args(self) -> dict:
        if self._kms_key_id:
            return {
                "ServerSideEncryption": "aws:kms",
                "SSEKMSKeyId": self._kms_key_id,
            }
        return {"ServerSideEncryption": "AES256"}

    def generate_presigned_url(self, s3_key: str, expiry_seconds: int = 3600) -> str:
        """Generate a pre-signed GET URL for *s3_key*."""
        return self._s3.generate_presigned_url(
            "get_object",
            Params={"Bucket": self._bucket, "Key": s3_key},
            ExpiresIn=expiry_seconds,
        )

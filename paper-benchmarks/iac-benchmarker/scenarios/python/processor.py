"""Main entry point for the document processing pipeline.

Wires together configuration, S3, SQS, DynamoDB, and SNS to form
the end-to-end document ingestion and processing workflow.

Usage:
    python processor.py

Environment variables (see config.py for full list):
    DOCUMENT_BUCKET      — S3 bucket for uploads and processed output
    DOCUMENT_QUEUE_URL   — SQS queue URL
    METADATA_TABLE       — DynamoDB table name
    RESULTS_TOPIC_ARN    — SNS topic ARN for result notifications
"""

import json
import logging
import sys
from datetime import datetime, timezone
from typing import Any
from uuid import uuid4

import boto3

from config import load_config, load_secrets
from dynamodb_store import DocumentMetadataStore
from s3_client import S3Client
from sqs_worker import SqsWorker

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)-8s %(name)s — %(message)s",
    datefmt="%Y-%m-%dT%H:%M:%S",
)
logger = logging.getLogger(__name__)


class DocumentProcessor:
    """Orchestrates the full document processing pipeline."""

    def __init__(self):
        self.cfg = load_config()
        self.cfg = load_secrets(self.cfg)

        logging.getLogger().setLevel(self.cfg.log_level)

        self.s3 = S3Client(self.cfg.document_bucket, self.cfg.encryption_key_id)
        self.store = DocumentMetadataStore(self.cfg.metadata_table)
        self.sns = boto3.client("sns")

        self.worker = SqsWorker(
            queue_url=self.cfg.document_queue_url,
            handler=self.handle_message,
            max_messages=self.cfg.max_messages,
            visibility_timeout=self.cfg.visibility_timeout,
            wait_time_seconds=self.cfg.wait_time_seconds,
        )

    # ------------------------------------------------------------------
    # Message handler (called by SqsWorker for each SQS message)
    # ------------------------------------------------------------------

    def handle_message(self, body: dict) -> None:
        """Process a single SQS message representing an S3 upload event."""
        # S3 event notifications arrive wrapped in an SNS envelope or directly.
        records = body.get("Records", [body])
        for record in records:
            s3_info = record.get("s3", {})
            s3_key = s3_info.get("object", {}).get("key", "")
            size_bytes = s3_info.get("object", {}).get("size", 0)
            if not s3_key:
                logger.warning("Message has no s3.object.key — skipping: %s", record)
                continue
            self._process_document(s3_key, size_bytes)

    # ------------------------------------------------------------------
    # Core processing logic
    # ------------------------------------------------------------------

    def _process_document(self, s3_key: str, size_bytes: int) -> None:
        document_id = str(uuid4())
        created_at = datetime.now(timezone.utc).isoformat()
        content_type = _infer_content_type(s3_key)

        logger.info("Processing document: s3_key=%s document_id=%s", s3_key, document_id)

        # 1. Record initial metadata.
        self.store.put_document(
            document_id=document_id,
            s3_key=s3_key,
            content_type=content_type,
            size_bytes=size_bytes,
            status="PROCESSING",
        )

        try:
            # 2. Download the raw document from S3.
            raw_bytes = self.s3.download_bytes(s3_key)

            # 3. Transform / extract text (stub — replace with real logic).
            processed_bytes, word_count = _extract_text(raw_bytes, content_type)

            # 4. Upload processed output.
            output_key = f"{self.cfg.output_prefix}{document_id}.txt"
            self.s3.upload_bytes(processed_bytes, output_key, content_type="text/plain")

            # 5. Update metadata with success.
            self.store.update_status(
                document_id, created_at, "COMPLETED",
            )
            self.store.put_document(
                document_id=document_id,
                s3_key=s3_key,
                content_type=content_type,
                size_bytes=size_bytes,
                status="COMPLETED",
                extra={"output_key": output_key, "word_count": word_count},
            )

            # 6. Publish result notification.
            self._publish_result(document_id, s3_key, output_key, word_count)

        except Exception as exc:  # pylint: disable=broad-except
            logger.error("Failed to process document %s: %s", document_id, exc, exc_info=True)
            self.store.update_status(document_id, created_at, "FAILED", str(exc))
            raise

    def _publish_result(
        self, document_id: str, input_key: str, output_key: str, word_count: int
    ) -> None:
        message = json.dumps({
            "document_id": document_id,
            "input_key": input_key,
            "output_key": output_key,
            "word_count": word_count,
            "timestamp": datetime.now(timezone.utc).isoformat(),
        })
        self.sns.publish(
            TopicArn=self.cfg.results_topic_arn,
            Message=message,
            Subject=f"Document processed: {document_id}",
            MessageAttributes={
                "document_id": {"DataType": "String", "StringValue": document_id},
            },
        )
        logger.info("Published result for document %s to SNS", document_id)

    # ------------------------------------------------------------------
    # Run
    # ------------------------------------------------------------------

    def run(self) -> None:
        logger.info("Document processor starting up")
        self.worker.run()


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _infer_content_type(s3_key: str) -> str:
    ext = s3_key.rsplit(".", 1)[-1].lower() if "." in s3_key else ""
    return {
        "pdf": "application/pdf",
        "txt": "text/plain",
        "docx": "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "html": "text/html",
        "json": "application/json",
    }.get(ext, "application/octet-stream")


def _extract_text(raw: bytes, content_type: str) -> tuple[bytes, int]:
    """Stub text extractor — returns raw bytes and approximate word count."""
    try:
        text = raw.decode("utf-8", errors="replace")
    except Exception:
        text = ""
    words = text.split()
    return text.encode("utf-8"), len(words)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    try:
        DocumentProcessor().run()
    except KeyboardInterrupt:
        logger.info("Interrupted — exiting.")
        sys.exit(0)
    except Exception as exc:
        logger.critical("Fatal error: %s", exc, exc_info=True)
        sys.exit(1)

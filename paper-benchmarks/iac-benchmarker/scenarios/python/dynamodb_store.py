"""DynamoDB store for document metadata.

Provides typed read/write operations for the `document-metadata` table.
The table uses `document_id` (String) as the partition key and
`created_at` (String, ISO-8601) as the sort key.
"""

import logging
from datetime import datetime, timezone
from typing import Any, Optional
from uuid import uuid4

import boto3
from boto3.dynamodb.conditions import Attr, Key
from botocore.exceptions import ClientError

logger = logging.getLogger(__name__)


class DocumentMetadataStore:
    """CRUD operations for the document-metadata DynamoDB table."""

    def __init__(self, table_name: str = "document-metadata", region: Optional[str] = None):
        kwargs: dict[str, Any] = {}
        if region:
            kwargs["region_name"] = region
        dynamodb = boto3.resource("dynamodb", **kwargs)
        self._table = dynamodb.Table(table_name)
        self._table_name = table_name

    # ------------------------------------------------------------------
    # Write
    # ------------------------------------------------------------------

    def put_document(
        self,
        document_id: str,
        s3_key: str,
        content_type: str,
        size_bytes: int,
        status: str = "PENDING",
        extra: Optional[dict] = None,
    ) -> dict:
        """Insert or overwrite a document metadata record."""
        now = datetime.now(timezone.utc).isoformat()
        item: dict[str, Any] = {
            "document_id": document_id,
            "created_at": now,
            "s3_key": s3_key,
            "content_type": content_type,
            "size_bytes": size_bytes,
            "status": status,
            "updated_at": now,
        }
        if extra:
            item.update(extra)

        self._table.put_item(Item=item)
        logger.info("Stored metadata for document %s (status=%s)", document_id, status)
        return item

    def update_status(
        self,
        document_id: str,
        created_at: str,
        status: str,
        error_message: Optional[str] = None,
    ) -> None:
        """Update the processing status of an existing document record."""
        update_expr = "SET #st = :status, updated_at = :ts"
        expr_names = {"#st": "status"}
        expr_values: dict[str, Any] = {
            ":status": status,
            ":ts": datetime.now(timezone.utc).isoformat(),
        }
        if error_message:
            update_expr += ", error_message = :err"
            expr_values[":err"] = error_message

        self._table.update_item(
            Key={"document_id": document_id, "created_at": created_at},
            UpdateExpression=update_expr,
            ExpressionAttributeNames=expr_names,
            ExpressionAttributeValues=expr_values,
        )
        logger.debug("Updated document %s status → %s", document_id, status)

    # ------------------------------------------------------------------
    # Read
    # ------------------------------------------------------------------

    def get_document(self, document_id: str, created_at: str) -> Optional[dict]:
        """Fetch a single document record by primary key."""
        resp = self._table.get_item(
            Key={"document_id": document_id, "created_at": created_at}
        )
        return resp.get("Item")

    def list_by_status(self, status: str) -> list[dict]:
        """Scan the table for all documents with the given status.

        Note: uses a Scan — suitable for small tables or infrequent admin use.
        For high-throughput queries, add a GSI on `status`.
        """
        resp = self._table.scan(
            FilterExpression=Attr("status").eq(status)
        )
        items = resp.get("Items", [])
        while "LastEvaluatedKey" in resp:
            resp = self._table.scan(
                FilterExpression=Attr("status").eq(status),
                ExclusiveStartKey=resp["LastEvaluatedKey"],
            )
            items.extend(resp.get("Items", []))
        return items

    def query_by_document_id(self, document_id: str) -> list[dict]:
        """Return all versions/events for a given document_id."""
        resp = self._table.query(
            KeyConditionExpression=Key("document_id").eq(document_id)
        )
        return resp.get("Items", [])

    # ------------------------------------------------------------------
    # Delete
    # ------------------------------------------------------------------

    def delete_document(self, document_id: str, created_at: str) -> None:
        """Delete a document metadata record."""
        self._table.delete_item(
            Key={"document_id": document_id, "created_at": created_at}
        )
        logger.info("Deleted metadata for document %s", document_id)

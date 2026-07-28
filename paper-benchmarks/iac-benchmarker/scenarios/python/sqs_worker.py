"""SQS polling worker for the document processing pipeline.

Polls the document queue in a long-poll loop, dispatches each message
to the registered handler, and deletes successfully processed messages.
Failed messages are left in the queue to be retried or sent to the DLQ.
"""

import json
import logging
import signal
import time
from typing import Callable, Optional

import boto3
from botocore.exceptions import ClientError

logger = logging.getLogger(__name__)

# Type alias for message handler callbacks.
MessageHandler = Callable[[dict], None]


class SqsWorker:
    """Long-poll SQS consumer with graceful shutdown support."""

    def __init__(
        self,
        queue_url: str,
        handler: MessageHandler,
        max_messages: int = 10,
        visibility_timeout: int = 300,
        wait_time_seconds: int = 20,
    ):
        self._queue_url = queue_url
        self._handler = handler
        self._max_messages = max_messages
        self._visibility_timeout = visibility_timeout
        self._wait_time_seconds = wait_time_seconds
        self._sqs = boto3.client("sqs")
        self._running = False

    # ------------------------------------------------------------------
    # Main loop
    # ------------------------------------------------------------------

    def run(self) -> None:
        """Start the polling loop.  Blocks until stop() is called or SIGTERM."""
        self._running = True
        signal.signal(signal.SIGTERM, self._handle_signal)
        signal.signal(signal.SIGINT, self._handle_signal)

        logger.info("SQS worker starting — queue: %s", self._queue_url)
        while self._running:
            try:
                self._poll_once()
            except Exception as exc:  # pylint: disable=broad-except
                logger.error("Unexpected error in poll loop: %s", exc, exc_info=True)
                time.sleep(5)

        logger.info("SQS worker stopped.")

    def stop(self) -> None:
        """Signal the polling loop to exit after the current batch."""
        logger.info("SQS worker shutting down ...")
        self._running = False

    # ------------------------------------------------------------------
    # Single poll cycle
    # ------------------------------------------------------------------

    def _poll_once(self) -> None:
        resp = self._sqs.receive_message(
            QueueUrl=self._queue_url,
            MaxNumberOfMessages=self._max_messages,
            VisibilityTimeout=self._visibility_timeout,
            WaitTimeSeconds=self._wait_time_seconds,
            AttributeNames=["All"],
            MessageAttributeNames=["All"],
        )
        messages = resp.get("Messages", [])
        if not messages:
            return

        logger.info("Received %d message(s)", len(messages))
        for msg in messages:
            self._process_message(msg)

    def _process_message(self, msg: dict) -> None:
        receipt_handle = msg["ReceiptHandle"]
        message_id = msg.get("MessageId", "?")
        try:
            body = json.loads(msg["Body"])
            logger.debug("Processing message %s: %s", message_id, body)
            self._handler(body)
            self._delete_message(receipt_handle)
            logger.info("Message %s processed and deleted", message_id)
        except json.JSONDecodeError as exc:
            logger.error("Message %s has invalid JSON body: %s", message_id, exc)
            # Leave in queue — will go to DLQ after maxReceiveCount.
        except Exception as exc:  # pylint: disable=broad-except
            logger.error("Handler failed for message %s: %s", message_id, exc, exc_info=True)
            # Leave in queue for retry.

    def _delete_message(self, receipt_handle: str) -> None:
        self._sqs.delete_message(
            QueueUrl=self._queue_url,
            ReceiptHandle=receipt_handle,
        )

    def change_visibility(self, receipt_handle: str, timeout: int) -> None:
        """Extend the visibility timeout for a message being processed."""
        self._sqs.change_message_visibility(
            QueueUrl=self._queue_url,
            ReceiptHandle=receipt_handle,
            VisibilityTimeout=timeout,
        )

    # ------------------------------------------------------------------
    # Signal handling
    # ------------------------------------------------------------------

    def _handle_signal(self, signum: int, _frame: Optional[object]) -> None:
        logger.info("Received signal %d — initiating graceful shutdown", signum)
        self.stop()

    # ------------------------------------------------------------------
    # Queue introspection
    # ------------------------------------------------------------------

    def get_queue_depth(self) -> int:
        """Return the approximate number of messages in the queue."""
        try:
            resp = self._sqs.get_queue_attributes(
                QueueUrl=self._queue_url,
                AttributeNames=["ApproximateNumberOfMessages"],
            )
            return int(resp["Attributes"].get("ApproximateNumberOfMessages", 0))
        except ClientError as exc:
            logger.warning("Could not get queue depth: %s", exc)
            return -1

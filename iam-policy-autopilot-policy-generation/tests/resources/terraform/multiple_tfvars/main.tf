resource "aws_s3_bucket" "data" {
  bucket = var.bucket_name
}

resource "aws_dynamodb_table" "items" {
  name     = var.table_name
  hash_key = "item_id"

  attribute {
    name = "item_id"
    type = "S"
  }
}

resource "aws_sqs_queue" "tasks" {
  name = var.queue_name
}

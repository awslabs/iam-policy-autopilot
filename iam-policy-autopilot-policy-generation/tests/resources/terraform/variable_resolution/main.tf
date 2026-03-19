resource "aws_s3_bucket" "data_bucket" {
  bucket = "${var.app_name}-${var.environment}-data"
}

resource "aws_dynamodb_table" "users" {
  name     = var.table_name
  hash_key = "user_id"

  attribute {
    name = "user_id"
    type = "S"
  }
}

resource "aws_sqs_queue" "tasks" {
  name = "${var.app_name}-tasks"
}

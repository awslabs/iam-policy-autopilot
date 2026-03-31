resource "aws_kinesis_stream" "events" {
  name             = "app-events-stream"
  shard_count      = 2
  retention_period = 24
}

resource "aws_sns_topic" "notifications" {
  name = "app-notifications"
}

resource "aws_s3_bucket" "archive" {
  bucket = "app-event-archive"
}

resource "aws_s3_bucket" "staging" {
  bucket = "app-staging-data"
}

resource "aws_s3_bucket" "results" {
  bucket = "app-results"
}

resource "aws_dynamodb_table" "events_log" {
  name     = "events-log"
  hash_key = "event_id"

  attribute {
    name = "event_id"
    type = "S"
  }
}

resource "aws_lambda_function" "processor" {
  function_name = "event-processor"
  handler       = "main.handler"
  runtime       = "python3.12"
  filename      = "processor.zip"

  role = aws_iam_role.processor_role.arn
}

resource "aws_iam_role" "processor_role" {
  name = "event-processor-role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action    = "sts:AssumeRole"
      Effect    = "Allow"
      Principal = { Service = "lambda.amazonaws.com" }
    }]
  })
}

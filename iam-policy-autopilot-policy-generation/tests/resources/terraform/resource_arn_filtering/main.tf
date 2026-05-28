provider "aws" {
  region = "us-east-1"
}

resource "aws_s3_bucket" "data_bucket" {
  bucket = "my-app-data-bucket"
}

resource "aws_s3_bucket" "logs_bucket" {
  bucket = "my-app-logs-bucket"
}

resource "aws_dynamodb_table" "users_table" {
  name         = "users-table"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "user_id"

  attribute {
    name = "user_id"
    type = "S"
  }
}

resource "aws_sqs_queue" "task_queue" {
  name = "task-processing-queue"
}

resource "aws_lambda_function" "processor" {
  function_name = "data-processor"
  handler       = "handler.main"
  runtime       = "python3.12"
  filename      = "lambda"

  role = aws_iam_role.lambda_role.arn
}

resource "aws_iam_role" "lambda_role" {
  name = "data-processor-role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Action = "sts:AssumeRole"
        Effect = "Allow"
        Principal = {
          Service = "lambda.amazonaws.com"
        }
      }
    ]
  })
}

data "aws_caller_identity" "current" {}

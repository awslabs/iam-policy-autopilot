resource "aws_s3_bucket" "data" {
  bucket = "my-data-bucket"
}

resource "aws_dynamodb_table" "items" {
  name     = "items-table"
  hash_key = "item_id"

  attribute {
    name = "item_id"
    type = "S"
  }
}

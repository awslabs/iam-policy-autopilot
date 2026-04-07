resource "aws_s3_bucket" "concrete" {
  bucket = "known-bucket"
}

resource "aws_s3_bucket" "dynamic" {
  bucket = var.bucket_name
}

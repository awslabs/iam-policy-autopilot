resource "google_storage_bucket" "gcs" {
  name = "my-gcs-bucket"
}

resource "aws_s3_bucket" "s3" {
  bucket = "my-s3-bucket"
}

resource "azurerm_storage_account" "azure" {
  name = "myazurestorage"
}

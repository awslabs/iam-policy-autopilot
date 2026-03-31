variable "app_name" {
  default = "myapp"
}

variable "environment" {
  default = "dev"
}

variable "table_name" {
  type = string
  # no default — must come from tfvars
}

variable "aws_region" {
  description = "AWS region"
  type        = string
  default     = "eu-central-1"
}

variable "instance_type" {
  description = "EC2 instance type for the Solana validator"
  type        = string
  default     = "m7i.4xlarge"
}

variable "key_name" {
  description = "Name of an existing AWS key pair to use for SSH"
  type        = string
}

variable "solana_version" {
  description = "Solana/Agave version to install"
  type        = string
  default     = "v1.18.26"
}

variable "ledger_disk_size_gb" {
  description = "Size of the EBS disk for ledger/accounts"
  type        = number
  default     = 1000
}

variable "allowed_ssh_cidr" {
  description = "CIDR allowed to SSH into the validator"
  type        = string
  default     = "0.0.0.0/0"
}

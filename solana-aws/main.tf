terraform {
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

provider "aws" {
  region = var.aws_region
}

data "aws_ami" "ubuntu" {
  most_recent = true
  owners      = ["099720109477"] # Canonical

  filter {
    name   = "name"
    values = ["ubuntu/images/hvm-ssd/ubuntu-jammy-22.04-amd64-server-*"]
  }
}

data "aws_vpc" "default" {
  default = true
}

data "aws_subnets" "default" {
  filter {
    name   = "vpc-id"
    values = [data.aws_vpc.default.id]
  }

  filter {
    name   = "default-for-az"
    values = ["true"]
  }
}

resource "aws_security_group" "solana_validator_sg" {
  name        = "solana-validator-sg"
  description = "Security group for Solana validator"
  vpc_id      = data.aws_vpc.default.id

  ingress {
    description = "SSH"
    from_port   = 22
    to_port     = 22
    protocol    = "tcp"
    cidr_blocks = [var.allowed_ssh_cidr]
  }

  ingress {
    description = "Solana TCP range"
    from_port   = 8000
    to_port     = 8020
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  ingress {
    description = "Solana UDP range"
    from_port   = 8000
    to_port     = 8020
    protocol    = "udp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  ingress {
    description = "Solana RPC"
    from_port   = 8899
    to_port     = 8899
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  ingress {
    description = "Prometheus exporter"
    from_port   = 9101
    to_port     = 9101
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = {
    Name = "solana-validator-sg"
  }
}

locals {
  user_data = file("${path.module}/user-data.sh")
}

resource "aws_instance" "solana_validator" {
  ami                         = data.aws_ami.ubuntu.id
  instance_type               = var.instance_type
  subnet_id                   = data.aws_subnets.default.ids[0]
  vpc_security_group_ids      = [aws_security_group.solana_validator_sg.id]
  associate_public_ip_address = true
  key_name                    = var.key_name
  user_data                   = local.user_data

  root_block_device {
    volume_size = 80
    volume_type = "gp3"
  }

  ebs_block_device {
    device_name = "/dev/sdf"
    volume_size = var.ledger_disk_size_gb
    volume_type = "gp3"
  }

  tags = {
    Name = "solana-mainnet-nonvoting"
  }
}

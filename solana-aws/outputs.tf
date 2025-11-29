output "validator_public_ip" {
  description = "Public IP of the Agave validator"
  value       = aws_instance.solana_validator.public_ip
}

output "validator_identity_pubkey_hint" {
  description = "Check /root/validator-identity.txt on the instance for the Agave identity pubkey"
  value       = "ssh -i <your-key.pem> ubuntu@${aws_instance.solana_validator.public_ip} && cat /root/validator-identity.txt"
}

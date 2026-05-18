terraform {
  required_version = ">= 1.0"
  required_providers {
    nebius = {
      source  = "terraform-provider.storage.eu-north1.nebius.cloud/nebius/nebius"
      version = ">= 0.5.55"
    }
  }
}

# Authentication is provided via the NEBIUS_IAM_TOKEN environment variable,
# which the nasiko CLI exports automatically by calling
# `nebius iam get-access-token`. The token field accepts the same value.
provider "nebius" {
  token = var.nb_token != "" ? var.nb_token : null
}

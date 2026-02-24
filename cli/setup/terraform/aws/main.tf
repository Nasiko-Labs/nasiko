# 1. Configure the AWS Provider
provider "aws" {
  region = var.aws_region
}

# 2. Get available availability zones
data "aws_availability_zones" "available" {
  state = "available"
}

# Calculate how many AZs to use (max 3, but some regions have fewer)
locals {
  az_count        = min(3, length(data.aws_availability_zones.available.names))
  azs             = slice(data.aws_availability_zones.available.names, 0, local.az_count)
  private_subnets = slice(["10.0.1.0/24", "10.0.2.0/24", "10.0.3.0/24"], 0, local.az_count)
  public_subnets  = slice(["10.0.101.0/24", "10.0.102.0/24", "10.0.103.0/24"], 0, local.az_count)
}

# 3. Create a new VPC
module "vpc" {
  source  = "terraform-aws-modules/vpc/aws"
  version = "5.8.1"

  name = "${var.cluster_name}-vpc"
  cidr = "10.0.0.0/16"

  azs             = local.azs
  private_subnets = local.private_subnets
  public_subnets  = local.public_subnets

  enable_nat_gateway   = true
  single_nat_gateway   = true
  enable_dns_hostnames = true

  public_subnet_tags = {
    "kubernetes.io/cluster/${var.cluster_name}" = "shared"
    "kubernetes.io/role/elb"                    = "1"
  }

  private_subnet_tags = {
    "kubernetes.io/cluster/${var.cluster_name}" = "shared"
    "kubernetes.io/role/internal-elb"           = "1"
  }
}

# 4. Create the EKS cluster using the official module
module "eks" {
  source  = "terraform-aws-modules/eks/aws"
  version = "21.8.0" # v21.x requires new variable names

  name = var.cluster_name

  kubernetes_version = var.cluster_version

  vpc_id     = module.vpc.vpc_id
  subnet_ids = module.vpc.private_subnets

  endpoint_public_access  = true
  endpoint_private_access = true

  # --- AUTHENTICATION (Standard for v21) ---
  enable_cluster_creator_admin_permissions = true
  authentication_mode                      = "API_AND_CONFIG_MAP"

  # Configure EKS addons - critical for node networking
  addons = {
    vpc-cni = {
      before_compute              = true # CRITICAL: Must be installed before nodes
      most_recent                 = true
      preserve                    = true
      resolve_conflicts_on_create = "OVERWRITE"
      resolve_conflicts_on_update = "OVERWRITE"
    }
    eks-pod-identity-agent = {
      before_compute = true # Required for pod identity functionality
    }
    aws-ebs-csi-driver = {
      most_recent                 = true
      preserve                    = true
      before_compute              = false # Install after nodes are ready
      resolve_conflicts_on_create = "OVERWRITE"
      resolve_conflicts_on_update = "OVERWRITE"
      service_account_role_arn    = module.ebs_csi_driver_role.arn
    }
    coredns = {
      most_recent                 = true
      preserve                    = true
      before_compute              = false # Install after nodes are ready
      resolve_conflicts_on_create = "OVERWRITE"
      resolve_conflicts_on_update = "OVERWRITE"
    }
    kube-proxy = {
      before_compute              = true # CRITICAL: Required for node networking
      most_recent                 = true
      preserve                    = true
      resolve_conflicts_on_create = "OVERWRITE"
      resolve_conflicts_on_update = "OVERWRITE"
    }
  }

  # This creates the default worker node group
  eks_managed_node_groups = {
    default_nodes = {
      # Use AL2023 for K8s 1.30+
      ami_type       = "AL2023_x86_64_STANDARD"
      instance_types = [var.instance_type]

      min_size     = 1
      max_size     = 3
      desired_size = 2
      disk_size    = 50

      # subnet_ids inherited from module-level configuration (line 48)
      # No need to specify here unless using different subnets for this node group

      labels = {
        Environment = "development"
        NodeGroup   = "default"
      }

      tags = {
        Environment = "development"
        ManagedBy   = "Terraform"
      }
    }
  }

  tags = {
    Environment = "development"
    ManagedBy   = "Terraform"
  }
}

# 5. IAM Role for BuildKit
module "buildkit_iam_role" {
  source  = "terraform-aws-modules/iam/aws//modules/iam-role-for-service-accounts"
  version = "~> 6.0"

  name            = "${var.cluster_name}-buildkit-role"
  use_name_prefix = false

  oidc_providers = {
    main = {
      provider_arn               = module.eks.oidc_provider_arn
      namespace_service_accounts = ["buildkit:buildkit-sa"]
    }
  }

  policies = {
    AmazonEC2ContainerRegistryPowerUser = "arn:aws:iam::aws:policy/AmazonEC2ContainerRegistryPowerUser"
  }
}

data "http" "aws_lb_controller_policy" {
  url = "https://raw.githubusercontent.com/kubernetes-sigs/aws-load-balancer-controller/main/docs/install/iam_policy.json"
}

resource "aws_iam_policy" "load_balancer_controller" {
  name        = "${var.cluster_name}-aws-lbc-policy"
  description = "AWS Load Balancer Controller policy for ${var.cluster_name}"
  policy      = data.http.aws_lb_controller_policy.response_body
}

# 6. IAM Role for EBS CSI Driver
module "ebs_csi_driver_role" {
  source  = "terraform-aws-modules/iam/aws//modules/iam-role-for-service-accounts"
  version = "~> 6.0"

  name            = "${var.cluster_name}-ebs-csi-driver-role"
  use_name_prefix = false

  # Disable built-in policy creation
  attach_ebs_csi_policy = false

  # Attach standard AWS managed policy using "policies" map
  policies = {
    AmazonEBSCSIDriverPolicy = "arn:aws:iam::aws:policy/service-role/AmazonEBSCSIDriverPolicy"
  }

  oidc_providers = {
    main = {
      provider_arn               = module.eks.oidc_provider_arn
      namespace_service_accounts = ["kube-system:ebs-csi-controller-sa"]
    }
  }
}

# 7. IAM Role for AWS Load Balancer Controller
module "aws_load_balancer_controller_role" {
  source  = "terraform-aws-modules/iam/aws//modules/iam-role-for-service-accounts"
  version = "~> 6.0"

  name            = "${var.cluster_name}-aws-load-balancer-controller"
  use_name_prefix = false

  # Disable built-in policy creation
  attach_load_balancer_controller_policy = false

  # Attach our custom unique policy using "policies" map
  policies = {
    LoadBalancerControllerPolicy = aws_iam_policy.load_balancer_controller.arn
  }

  oidc_providers = {
    main = {
      provider_arn               = module.eks.oidc_provider_arn
      namespace_service_accounts = ["kube-system:aws-load-balancer-controller"]
    }
  }
}

output "buildkit_role_arn" {
  description = "IAM Role ARN for BuildKit"
  value       = module.buildkit_iam_role.arn
}

output "ebs_csi_driver_role_arn" {
  description = "IAM Role ARN for EBS CSI Driver"
  value       = module.ebs_csi_driver_role.arn
}

output "aws_load_balancer_controller_role_arn" {
  description = "IAM Role ARN for AWS Load Balancer Controller"
  value       = module.aws_load_balancer_controller_role.arn
}

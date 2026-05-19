variable "nb_token" {
  type        = string
  description = "Nebius IAM access token. If empty, the provider reads NEBIUS_IAM_TOKEN from the environment."
  sensitive   = true
  default     = ""
}

variable "nb_project_id" {
  type        = string
  description = "Nebius project ID (parent_id of the cluster)."
}

variable "nb_subnet_id" {
  type        = string
  description = "Nebius VPC subnet ID used by the cluster control plane and node group."
}

variable "nb_region" {
  type        = string
  description = "Nebius region (e.g. us-central1, eu-north1, eu-west1). Used for labelling only; the actual region is derived from the subnet."
  default     = "us-central1"
}

variable "cluster_name" {
  type        = string
  description = "Managed Kubernetes cluster name."
  default     = "nasiko"
}

variable "k8s_version" {
  type        = string
  description = "Kubernetes minor version (e.g. \"1.32\")."
  default     = "1.32"
}

variable "node_count" {
  type        = number
  description = "Number of worker nodes in the default node group."
  default     = 3
}

variable "node_platform" {
  type        = string
  description = "Compute platform for worker nodes (e.g. cpu-d3 for AMD EPYC Genoa, cpu-e2 for Intel Ice Lake)."
  default     = "cpu-d3"
}

variable "node_preset" {
  type        = string
  description = "Compute preset for worker nodes (e.g. 4vcpu-16gb)."
  default     = "4vcpu-16gb"
}

variable "boot_disk_gib" {
  type        = number
  description = "Boot disk size in GiB for each worker node."
  default     = 64
}

variable "etcd_size" {
  type        = number
  description = "Number of etcd nodes in the control plane (3 = HA, 1 = single-instance)."
  default     = 1
}

variable "service_cidr" {
  type        = string
  description = "CIDR block (or prefix length like \"/20\") for Kubernetes Service ClusterIP allocation. Reserved from the subnet's VPC pool. Must fit inside the pool — small projects often have a /20 pool, so /24 is a safe default."
  default     = "/24"
}

variable "do_token" {
  type        = string
  description = "DigitalOcean API token"
  sensitive   = true
}

variable "do_region" {
  type        = string
  description = "DigitalOcean region"
  default     = "nyc3"
}

variable "cluster_name" {
  type        = string
  description = "DOKS cluster name"
  default     = "my-doks-cluster"
}

variable "kubernetes_version" {
  type        = string
  description = "Kubernetes version"
  default     = "1.28"
}

variable "node_pool_name" {
  type        = string
  description = "Node pool name"
  default     = "default-pool"
}

variable "node_count" {
  type        = number
  description = "Number of nodes in the pool"
  default     = 2
}

variable "node_size" {
  type        = string
  description = "Size of nodes (slug)"
  default     = "s-2vcpu-4gb"
}

variable "auto_scale_enabled" {
  type        = bool
  description = "Enable auto-scaling for node pool"
  default     = true
}

variable "auto_scale_min" {
  type        = number
  description = "Minimum nodes for auto-scaling"
  default     = 1
}

variable "auto_scale_max" {
  type        = number
  description = "Maximum nodes for auto-scaling"
  default     = 5
}

variable "tags" {
  type        = list(string)
  description = "Tags to assign to resources"
  default     = ["kubernetes", "doks"]
}

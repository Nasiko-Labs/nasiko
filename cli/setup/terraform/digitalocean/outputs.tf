output "cluster_name" {
  value       = digitalocean_kubernetes_cluster.main.name
  description = "DOKS cluster name"
}

output "cluster_id" {
  value       = digitalocean_kubernetes_cluster.main.id
  description = "DOKS cluster ID"
}

output "cluster_urn" {
  value       = digitalocean_kubernetes_cluster.main.urn
  description = "DOKS cluster URN"
}

output "kubernetes_version" {
  value       = digitalocean_kubernetes_cluster.main.version
  description = "Kubernetes version running on the cluster"
}

output "cluster_status" {
  value       = digitalocean_kubernetes_cluster.main.status
  description = "Cluster status"
}

output "endpoint" {
  value       = digitalocean_kubernetes_cluster.main.endpoint
  description = "Kubernetes API server endpoint"
}

output "ipv4_address" {
  value       = digitalocean_kubernetes_cluster.main.ipv4_address
  description = "IPv4 address of the cluster"
}

output "cluster_ca_certificate" {
  value       = digitalocean_kubernetes_cluster.main.kube_config[0].cluster_ca_certificate
  sensitive   = true
  description = "Certificate Authority data for the cluster"
}

output "configure_kubectl" {
  value       = "doctl kubernetes cluster kubeconfig save ${digitalocean_kubernetes_cluster.main.id}"
  description = "Command to configure kubectl"
}

output "node_pools" {
  value = {
    default     = digitalocean_kubernetes_cluster.main.node_pool[0]
    additional  = digitalocean_kubernetes_node_pool.additional
  }
  description = "Node pools information"
}

output "kube_config" {
  value       = digitalocean_kubernetes_cluster.main.kube_config[0].raw_config
  sensitive   = true
  description = "Raw Kubeconfig YAML"
}

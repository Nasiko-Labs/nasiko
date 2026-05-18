output "cluster_id" {
  value       = nebius_mk8s_v1_cluster.main.id
  description = "Managed Kubernetes cluster ID."
}

output "cluster_name" {
  value       = nebius_mk8s_v1_cluster.main.name
  description = "Managed Kubernetes cluster name."
}

output "region" {
  value       = var.nb_region
  description = "Region the cluster was provisioned in."
}

output "public_endpoint" {
  value       = nebius_mk8s_v1_cluster.main.status.control_plane.endpoints.public_endpoint
  description = "Public DNS name or IP of the Kubernetes API server (exposed via load balancer)."
}

output "cluster_ca_certificate" {
  value       = nebius_mk8s_v1_cluster.main.status.control_plane.auth.cluster_ca_certificate
  description = "PEM-encoded cluster CA certificate."
  sensitive   = true
}

output "configure_kubectl" {
  value       = "nebius mk8s cluster get-credentials --id ${nebius_mk8s_v1_cluster.main.id} --external"
  description = "Command that writes a kubeconfig pointing at the public Kubernetes API endpoint."
}

output "node_group_id" {
  value       = nebius_mk8s_v1_node_group.default.id
  description = "Default node group ID."
}

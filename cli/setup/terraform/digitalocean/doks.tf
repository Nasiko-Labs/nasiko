data "digitalocean_kubernetes_versions" "current" {
  version_prefix = "1.34." # Pin to a major version (optional) or remove to get absolute latest
}

resource "digitalocean_kubernetes_cluster" "main" {
  name         = var.cluster_name
  region       = var.do_region
  version      = data.digitalocean_kubernetes_versions.current.latest_version
  auto_upgrade = true
  ha           = true
  surge_upgrade = true

  node_pool {
    name       = var.node_pool_name
    size       = var.node_size
    node_count = var.node_count
    auto_scale = var.auto_scale_enabled
    min_nodes  = var.auto_scale_min
    max_nodes  = var.auto_scale_max

    labels = {
      environment = "production"
      managed_by  = "terraform"
    }

    tags = var.tags
  }

  tags = var.tags
}

resource "digitalocean_kubernetes_node_pool" "additional" {
  cluster_id = digitalocean_kubernetes_cluster.main.id
  name       = "additional-pool"
  size       = var.node_size
  node_count = 1
  auto_scale = var.auto_scale_enabled
  min_nodes  = 1
  max_nodes  = 3

  labels = {
    environment = "production"
    pool_type   = "additional"
  }

  tags = var.tags
}

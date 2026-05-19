resource "nebius_mk8s_v1_cluster" "main" {
  parent_id = var.nb_project_id
  name      = var.cluster_name

  labels = {
    managed_by = "nasiko"
  }

  # The Service CIDR is reserved from the subnet's VPC pool. The provider
  # defaults to /16, which fails on small VPC pools (e.g. /20). We pin a
  # smaller prefix to make the module work on the common /20 pool layout.
  kube_network = {
    service_cidrs = [var.service_cidr]
  }

  control_plane = {
    subnet_id         = var.nb_subnet_id
    version           = var.k8s_version
    etcd_cluster_size = var.etcd_size

    # Allocate a public endpoint so the Kubernetes API is reachable from the
    # internet via the Nebius-managed load balancer. The empty object enables
    # the endpoint; removing this block would make the API private-only.
    endpoints = {
      public_endpoint = {}
    }
  }
}

resource "nebius_mk8s_v1_node_group" "default" {
  parent_id        = nebius_mk8s_v1_cluster.main.id
  name             = "${var.cluster_name}-default"
  fixed_node_count = var.node_count
  version          = var.k8s_version

  template = {
    resources = {
      platform = var.node_platform
      preset   = var.node_preset
    }

    boot_disk = {
      type           = "NETWORK_SSD"
      size_gibibytes = var.boot_disk_gib
    }

    network_interfaces = [{
      subnet_id = var.nb_subnet_id
    }]
  }
}

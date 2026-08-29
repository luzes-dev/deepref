variable "REGISTRY" {
  default = "localhost:5000"
}

variable "IMAGE_PREFIX" {
  default = "deepref"
}

variable "OCI_SOURCE" {
  default = "https://github.com/luzes-dev/deepref"
}

variable "OCI_REVISION" {
  default = "local"
}

variable "OCI_VERSION" {
  default = "0.0.0-dev"
}

variable "GIT_TREE_HASH" {
  default = "local"
}

variable "RUST_BASE_REPOSITORY" {
  default = "docker.io/library/rust"
}

variable "DEBIAN_BASE_REPOSITORY" {
  default = "docker.io/library/debian"
}

variable "NODE_BASE_REPOSITORY" {
  default = "docker.io/library/node"
}

variable "CADDY_BASE_REPOSITORY" {
  default = "docker.io/library/caddy"
}

group "default" {
  targets = ["api", "worker", "projector", "web"]
}

target "release" {
  context   = "."
  platforms = ["linux/amd64", "linux/arm64"]
  attest = [
    "type=provenance,mode=max",
    "type=sbom"
  ]
  labels = {
    "org.opencontainers.image.source"   = OCI_SOURCE
    "org.opencontainers.image.revision" = OCI_REVISION
    "org.opencontainers.image.version"  = OCI_VERSION
    "dev.deepref.git.tree"               = GIT_TREE_HASH
  }
}

target "rust-service" {
  inherits   = ["release"]
  dockerfile = "docker/rust-service.Dockerfile"
  args = {
    OCI_SOURCE             = OCI_SOURCE
    OCI_REVISION           = OCI_REVISION
    OCI_VERSION            = OCI_VERSION
    GIT_TREE_HASH          = GIT_TREE_HASH
    RUST_BASE_REPOSITORY   = RUST_BASE_REPOSITORY
    DEBIAN_BASE_REPOSITORY = DEBIAN_BASE_REPOSITORY
  }
}

target "api" {
  inherits = ["rust-service"]
  target   = "api"
  tags     = ["${REGISTRY}/${IMAGE_PREFIX}-api:${GIT_TREE_HASH}"]
}

target "worker" {
  inherits = ["rust-service"]
  target   = "worker"
  tags     = ["${REGISTRY}/${IMAGE_PREFIX}-worker:${GIT_TREE_HASH}"]
}

target "projector" {
  inherits = ["rust-service"]
  target   = "projector"
  tags     = ["${REGISTRY}/${IMAGE_PREFIX}-projector:${GIT_TREE_HASH}"]
}

target "web" {
  inherits   = ["release"]
  dockerfile = "apps/web/Dockerfile"
  target     = "runtime"
  tags       = ["${REGISTRY}/${IMAGE_PREFIX}-web:${GIT_TREE_HASH}"]
  args = {
    OCI_SOURCE            = OCI_SOURCE
    OCI_REVISION          = OCI_REVISION
    OCI_VERSION           = OCI_VERSION
    GIT_TREE_HASH         = GIT_TREE_HASH
    NODE_BASE_REPOSITORY  = NODE_BASE_REPOSITORY
    CADDY_BASE_REPOSITORY = CADDY_BASE_REPOSITORY
  }
}

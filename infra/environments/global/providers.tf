provider "aws" {
  region = var.aws_region

  default_tags {
    tags = merge(var.tags, {
      Environment = "global"
      ManagedBy   = "OpenTofu"
      Project     = var.project_name
    })
  }
}

provider "aws" {
  alias  = "development"
  region = var.aws_region

  assume_role {
    role_arn = var.aws_environments.development.cluster_access_role_arn
  }
}

provider "aws" {
  alias  = "staging"
  region = var.aws_region

  assume_role {
    role_arn = var.aws_environments.staging.cluster_access_role_arn
  }
}

provider "aws" {
  alias  = "production"
  region = var.aws_region

  assume_role {
    role_arn = var.aws_environments.production.cluster_access_role_arn
  }
}

data "aws_caller_identity" "global" {}
data "aws_caller_identity" "development" {
  provider = aws.development
}

data "aws_caller_identity" "staging" {
  provider = aws.staging
}

data "aws_caller_identity" "production" {
  provider = aws.production
}

data "aws_eks_cluster" "development" {
  provider = aws.development
  name     = var.aws_environments.development.eks_cluster_name
}

data "aws_eks_cluster" "staging" {
  provider = aws.staging
  name     = var.aws_environments.staging.eks_cluster_name
}

data "aws_eks_cluster" "production" {
  provider = aws.production
  name     = var.aws_environments.production.eks_cluster_name
}

provider "kubernetes" {
  alias                  = "development"
  host                   = data.aws_eks_cluster.development.endpoint
  cluster_ca_certificate = base64decode(data.aws_eks_cluster.development.certificate_authority[0].data)

  exec {
    api_version = "client.authentication.k8s.io/v1beta1"
    command     = "aws"
    args = [
      "eks", "get-token",
      "--cluster-name", var.aws_environments.development.eks_cluster_name,
      "--region", var.aws_region,
      "--role-arn", var.aws_environments.development.cluster_access_role_arn,
    ]
  }
}

provider "kubernetes" {
  alias                  = "staging"
  host                   = data.aws_eks_cluster.staging.endpoint
  cluster_ca_certificate = base64decode(data.aws_eks_cluster.staging.certificate_authority[0].data)

  exec {
    api_version = "client.authentication.k8s.io/v1beta1"
    command     = "aws"
    args = [
      "eks", "get-token",
      "--cluster-name", var.aws_environments.staging.eks_cluster_name,
      "--region", var.aws_region,
      "--role-arn", var.aws_environments.staging.cluster_access_role_arn,
    ]
  }
}

provider "kubernetes" {
  alias                  = "production"
  host                   = data.aws_eks_cluster.production.endpoint
  cluster_ca_certificate = base64decode(data.aws_eks_cluster.production.certificate_authority[0].data)

  exec {
    api_version = "client.authentication.k8s.io/v1beta1"
    command     = "aws"
    args = [
      "eks", "get-token",
      "--cluster-name", var.aws_environments.production.eks_cluster_name,
      "--region", var.aws_region,
      "--role-arn", var.aws_environments.production.cluster_access_role_arn,
    ]
  }
}

provider "helm" {
  alias = "development"
  kubernetes {
    host                   = data.aws_eks_cluster.development.endpoint
    cluster_ca_certificate = base64decode(data.aws_eks_cluster.development.certificate_authority[0].data)

    exec {
      api_version = "client.authentication.k8s.io/v1beta1"
      command     = "aws"
      args = [
        "eks", "get-token",
        "--cluster-name", var.aws_environments.development.eks_cluster_name,
        "--region", var.aws_region,
        "--role-arn", var.aws_environments.development.cluster_access_role_arn,
      ]
    }
  }
}

provider "helm" {
  alias = "staging"
  kubernetes {
    host                   = data.aws_eks_cluster.staging.endpoint
    cluster_ca_certificate = base64decode(data.aws_eks_cluster.staging.certificate_authority[0].data)

    exec {
      api_version = "client.authentication.k8s.io/v1beta1"
      command     = "aws"
      args = [
        "eks", "get-token",
        "--cluster-name", var.aws_environments.staging.eks_cluster_name,
        "--region", var.aws_region,
        "--role-arn", var.aws_environments.staging.cluster_access_role_arn,
      ]
    }
  }
}

provider "helm" {
  alias = "production"
  kubernetes {
    host                   = data.aws_eks_cluster.production.endpoint
    cluster_ca_certificate = base64decode(data.aws_eks_cluster.production.certificate_authority[0].data)

    exec {
      api_version = "client.authentication.k8s.io/v1beta1"
      command     = "aws"
      args = [
        "eks", "get-token",
        "--cluster-name", var.aws_environments.production.eks_cluster_name,
        "--region", var.aws_region,
        "--role-arn", var.aws_environments.production.cluster_access_role_arn,
      ]
    }
  }
}

provider "cloudflare" {}

provider "github" {
  owner = var.github_owner
}

mock_provider "aws" {}

run "three_az_private_network" {
  command = plan

  variables {
    name                 = "test-network"
    vpc_cidr             = "10.40.0.0/16"
    availability_zones   = ["sa-east-1a", "sa-east-1b", "sa-east-1c"]
    public_subnet_cidrs  = ["10.40.0.0/20", "10.40.16.0/20", "10.40.32.0/20"]
    private_subnet_cidrs = ["10.40.48.0/20", "10.40.64.0/20", "10.40.80.0/20"]
    data_subnet_cidrs    = ["10.40.96.0/20", "10.40.112.0/20", "10.40.128.0/20"]
    nat_gateway_mode     = "one_per_az"
  }

  assert {
    condition     = length(aws_nat_gateway.this) == 3
    error_message = "one_per_az must create three NAT gateways."
  }

  assert {
    condition     = length(aws_subnet.private) == 3 && length(aws_subnet.data) == 3
    error_message = "workload and data tiers must span all three AZs."
  }

  assert {
    condition     = length(aws_vpc_endpoint.interface) >= 6
    error_message = "required private AWS service endpoints must be present."
  }
}

run "development_shared_nat" {
  command = plan

  variables {
    name                 = "test-development"
    vpc_cidr             = "10.41.0.0/16"
    availability_zones   = ["sa-east-1a", "sa-east-1b", "sa-east-1c"]
    public_subnet_cidrs  = ["10.41.0.0/20", "10.41.16.0/20", "10.41.32.0/20"]
    private_subnet_cidrs = ["10.41.48.0/20", "10.41.64.0/20", "10.41.80.0/20"]
    data_subnet_cidrs    = ["10.41.96.0/20", "10.41.112.0/20", "10.41.128.0/20"]
    nat_gateway_mode     = "single"
  }

  assert {
    condition     = length(aws_nat_gateway.this) == 1
    error_message = "Development must use exactly one shared NAT gateway."
  }
}

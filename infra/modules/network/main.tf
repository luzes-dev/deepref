data "aws_region" "current" {}

locals {
  zones = {
    for index, zone in var.availability_zones : zone => {
      index        = index
      public_cidr  = var.public_subnet_cidrs[index]
      private_cidr = var.private_subnet_cidrs[index]
      data_cidr    = var.data_subnet_cidrs[index]
    }
  }

  nat_zones = var.nat_gateway_mode == "single" ? {
    (var.availability_zones[0]) = local.zones[var.availability_zones[0]]
  } : local.zones

  common_tags = merge(var.tags, { ManagedBy = "OpenTofu" })
}

resource "aws_vpc" "this" {
  cidr_block           = var.vpc_cidr
  enable_dns_hostnames = true
  enable_dns_support   = true

  tags = merge(local.common_tags, { Name = var.name })
}

resource "aws_internet_gateway" "this" {
  vpc_id = aws_vpc.this.id
  tags   = merge(local.common_tags, { Name = "${var.name}-igw" })
}

resource "aws_subnet" "public" {
  for_each = local.zones

  vpc_id                  = aws_vpc.this.id
  availability_zone       = each.key
  cidr_block              = each.value.public_cidr
  map_public_ip_on_launch = false

  tags = merge(local.common_tags, {
    Name                     = "${var.name}-public-${each.key}"
    Tier                     = "public"
    "kubernetes.io/role/elb" = "1"
  })
}

resource "aws_subnet" "private" {
  for_each = local.zones

  vpc_id                  = aws_vpc.this.id
  availability_zone       = each.key
  cidr_block              = each.value.private_cidr
  map_public_ip_on_launch = false

  tags = merge(local.common_tags, {
    Name                              = "${var.name}-private-${each.key}"
    Tier                              = "workload"
    "kubernetes.io/role/internal-elb" = "1"
  })
}

resource "aws_subnet" "data" {
  for_each = local.zones

  vpc_id                  = aws_vpc.this.id
  availability_zone       = each.key
  cidr_block              = each.value.data_cidr
  map_public_ip_on_launch = false

  tags = merge(local.common_tags, {
    Name = "${var.name}-data-${each.key}"
    Tier = "data"
  })
}

resource "aws_route_table" "public" {
  vpc_id = aws_vpc.this.id
  tags   = merge(local.common_tags, { Name = "${var.name}-public" })
}

resource "aws_route" "public_internet" {
  route_table_id         = aws_route_table.public.id
  destination_cidr_block = "0.0.0.0/0"
  gateway_id             = aws_internet_gateway.this.id
}

resource "aws_route_table_association" "public" {
  for_each = aws_subnet.public

  subnet_id      = each.value.id
  route_table_id = aws_route_table.public.id
}

resource "aws_eip" "nat" {
  for_each = local.nat_zones

  domain = "vpc"
  tags   = merge(local.common_tags, { Name = "${var.name}-nat-${each.key}" })

  depends_on = [aws_internet_gateway.this]
}

resource "aws_nat_gateway" "this" {
  for_each = local.nat_zones

  allocation_id = aws_eip.nat[each.key].id
  subnet_id     = aws_subnet.public[each.key].id
  tags          = merge(local.common_tags, { Name = "${var.name}-${each.key}" })

  depends_on = [aws_internet_gateway.this]
}

resource "aws_route_table" "private" {
  for_each = local.zones

  vpc_id = aws_vpc.this.id
  tags   = merge(local.common_tags, { Name = "${var.name}-private-${each.key}" })
}

resource "aws_route" "private_egress" {
  for_each = local.zones

  route_table_id         = aws_route_table.private[each.key].id
  destination_cidr_block = "0.0.0.0/0"
  nat_gateway_id = var.nat_gateway_mode == "single" ? (
    aws_nat_gateway.this[var.availability_zones[0]].id
  ) : aws_nat_gateway.this[each.key].id
}

resource "aws_route_table_association" "private" {
  for_each = aws_subnet.private

  subnet_id      = each.value.id
  route_table_id = aws_route_table.private[each.key].id
}

resource "aws_route_table" "data" {
  for_each = local.zones

  vpc_id = aws_vpc.this.id
  tags   = merge(local.common_tags, { Name = "${var.name}-data-${each.key}" })
}

resource "aws_route_table_association" "data" {
  for_each = aws_subnet.data

  subnet_id      = each.value.id
  route_table_id = aws_route_table.data[each.key].id
}

resource "aws_security_group" "endpoints" {
  name_prefix = "${var.name}-endpoints-"
  description = "TLS from the VPC to private AWS service endpoints"
  vpc_id      = aws_vpc.this.id

  ingress {
    description = "HTTPS from VPC"
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = [var.vpc_cidr]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = [var.vpc_cidr]
  }

  tags = merge(local.common_tags, { Name = "${var.name}-endpoints" })
}

resource "aws_vpc_endpoint" "interface" {
  for_each = var.interface_endpoint_services

  vpc_id              = aws_vpc.this.id
  service_name        = "com.amazonaws.${data.aws_region.current.region}.${each.value}"
  vpc_endpoint_type   = "Interface"
  private_dns_enabled = true
  subnet_ids          = [for zone in var.availability_zones : aws_subnet.private[zone].id]
  security_group_ids  = [aws_security_group.endpoints.id]

  tags = merge(local.common_tags, { Name = "${var.name}-${replace(each.value, ".", "-")}" })
}

resource "aws_vpc_endpoint" "s3" {
  vpc_id            = aws_vpc.this.id
  service_name      = "com.amazonaws.${data.aws_region.current.region}.s3"
  vpc_endpoint_type = "Gateway"
  route_table_ids = concat(
    [for zone in var.availability_zones : aws_route_table.private[zone].id],
    [for zone in var.availability_zones : aws_route_table.data[zone].id],
  )

  tags = merge(local.common_tags, { Name = "${var.name}-s3" })
}

resource "aws_cloudwatch_log_group" "flow" {
  name              = "/aws/vpc/${var.name}/flow"
  retention_in_days = var.flow_log_retention_days
  kms_key_id        = var.flow_log_kms_key_arn
  tags              = local.common_tags
}

resource "aws_iam_role" "flow" {
  name_prefix = "${var.name}-vpc-flow-"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "vpc-flow-logs.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
  tags = local.common_tags
}

resource "aws_iam_role_policy" "flow" {
  name = "publish-flow-logs"
  role = aws_iam_role.flow.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = [
        "logs:CreateLogStream",
        "logs:DescribeLogGroups",
        "logs:DescribeLogStreams",
        "logs:PutLogEvents",
      ]
      Resource = "${aws_cloudwatch_log_group.flow.arn}:*"
    }]
  })
}

resource "aws_flow_log" "this" {
  iam_role_arn    = aws_iam_role.flow.arn
  log_destination = aws_cloudwatch_log_group.flow.arn
  traffic_type    = "ALL"
  vpc_id          = aws_vpc.this.id

  tags = local.common_tags
}

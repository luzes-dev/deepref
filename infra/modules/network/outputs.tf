output "vpc_id" {
  description = "VPC identifier."
  value       = aws_vpc.this.id
}

output "vpc_cidr" {
  description = "VPC IPv4 CIDR."
  value       = aws_vpc.this.cidr_block
}

output "public_subnet_ids" {
  description = "Public subnet identifiers in availability-zone order."
  value       = [for zone in var.availability_zones : aws_subnet.public[zone].id]
}

output "private_subnet_ids" {
  description = "Private workload subnet identifiers in availability-zone order."
  value       = [for zone in var.availability_zones : aws_subnet.private[zone].id]
}

output "data_subnet_ids" {
  description = "Isolated data subnet identifiers in availability-zone order."
  value       = [for zone in var.availability_zones : aws_subnet.data[zone].id]
}

output "private_route_table_ids" {
  description = "Private route table identifiers."
  value       = [for zone in var.availability_zones : aws_route_table.private[zone].id]
}

output "data_route_table_ids" {
  description = "Data route table identifiers."
  value       = [for zone in var.availability_zones : aws_route_table.data[zone].id]
}

output "nat_gateway_ids" {
  description = "NAT gateway identifiers keyed by availability zone."
  value       = { for zone, gateway in aws_nat_gateway.this : zone => gateway.id }
}

output "endpoint_security_group_id" {
  description = "Security group attached to interface endpoints."
  value       = aws_security_group.endpoints.id
}

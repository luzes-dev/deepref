package deepref.helm.ownership

deny[msg] {
  input.kind == "Secret"
  msg := sprintf("Secret/%s must be produced by External Secrets, not rendered by Helm", [input.metadata.name])
}

deny[msg] {
  input.kind == "ServiceAccount"
  annotations := object.get(input.metadata, "annotations", {})
  object.get(annotations, "eks.amazonaws.com/role-arn", "") != ""
  msg := sprintf("ServiceAccount/%s must use EKS Pod Identity association owned by OpenTofu", [input.metadata.name])
}

deny[msg] {
  input.apiVersion == "external-secrets.io/v1beta1"
  input.kind == "ExternalSecret"
  object.get(input.spec.target, "creationPolicy", "") != "Owner"
  msg := sprintf("ExternalSecret/%s must own its generated Secret", [input.metadata.name])
}

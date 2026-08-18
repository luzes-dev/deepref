package deepref.helm.security

import future.keywords.in

workload_kinds := {"Deployment", "StatefulSet", "Job"}

deny[msg] {
  workload_kinds[input.kind]
  pod_spec := input.spec.template.spec
  object.get(object.get(pod_spec, "securityContext", {}), "runAsNonRoot", false) != true
  msg := sprintf("%s/%s must set pod runAsNonRoot", [input.kind, input.metadata.name])
}

deny[msg] {
  workload_kinds[input.kind]
  pod_spec := input.spec.template.spec
  object.get(object.get(object.get(pod_spec, "securityContext", {}), "seccompProfile", {}), "type", "") != "RuntimeDefault"
  msg := sprintf("%s/%s must use RuntimeDefault seccomp", [input.kind, input.metadata.name])
}

deny[msg] {
  workload_kinds[input.kind]
  container := input.spec.template.spec.containers[_]
  object.get(object.get(container, "securityContext", {}), "allowPrivilegeEscalation", true) != false
  msg := sprintf("%s/%s container %s permits privilege escalation", [input.kind, input.metadata.name, container.name])
}

deny[msg] {
  workload_kinds[input.kind]
  container := input.spec.template.spec.containers[_]
  object.get(object.get(container, "securityContext", {}), "readOnlyRootFilesystem", false) != true
  msg := sprintf("%s/%s container %s has a writable root filesystem", [input.kind, input.metadata.name, container.name])
}

deny[msg] {
  workload_kinds[input.kind]
  container := input.spec.template.spec.containers[_]
  drops := object.get(object.get(object.get(container, "securityContext", {}), "capabilities", {}), "drop", [])
  not "ALL" in drops
  msg := sprintf("%s/%s container %s must drop ALL capabilities", [input.kind, input.metadata.name, container.name])
}

deny[msg] {
  workload_kinds[input.kind]
  container := input.spec.template.spec.containers[_]
  requests := object.get(object.get(container, "resources", {}), "requests", {})
  not requests.cpu
  msg := sprintf("%s/%s container %s has no CPU request", [input.kind, input.metadata.name, container.name])
}

deny[msg] {
  workload_kinds[input.kind]
  container := input.spec.template.spec.containers[_]
  limits := object.get(object.get(container, "resources", {}), "limits", {})
  not limits.memory
  msg := sprintf("%s/%s container %s has no memory limit", [input.kind, input.metadata.name, container.name])
}

deny[msg] {
  input.kind == "Deployment"
  container := input.spec.template.spec.containers[_]
  not object.get(container, "livenessProbe", false)
  msg := sprintf("Deployment/%s container %s has no liveness probe", [input.metadata.name, container.name])
}

deny[msg] {
  input.kind == "StatefulSet"
  container := input.spec.template.spec.containers[_]
  not object.get(container, "readinessProbe", false)
  msg := sprintf("StatefulSet/%s container %s has no readiness probe", [input.metadata.name, container.name])
}

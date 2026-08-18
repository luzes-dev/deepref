package deepref.helm.immutable_images

deny[msg] {
  workload_spec := object.get(object.get(input, "spec", {}), "template", {})
  pod_spec := object.get(workload_spec, "spec", {})
  container := object.get(pod_spec, "containers", [])[_]
  not regex.match("^[^@[:space:]]+@sha256:[0-9a-f]{64}$", container.image)
  msg := sprintf("%s/%s container %s does not use an immutable image digest", [input.kind, input.metadata.name, container.name])
}

deny[msg] {
  input.kind == "Pod"
  container := input.spec.containers[_]
  not regex.match("^[^@[:space:]]+@sha256:[0-9a-f]{64}$", container.image)
  msg := sprintf("Pod/%s container %s does not use an immutable image digest", [input.metadata.name, container.name])
}

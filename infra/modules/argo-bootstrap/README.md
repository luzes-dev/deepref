# Argo bootstrap module

Installs one pinned `argo-cd` chart into a dedicated namespace and embeds exactly one root `Application` that reads the protected `gitops` branch. The server stays `ClusterIP`, chart ingress is disabled, and the built-in admin account is disabled. This module deliberately owns no workload namespace, ApplicationSet, application workload, External Secret, policy, collector, or repository credential.

The caller must provide authenticated Helm and Kubernetes providers for the private EKS endpoint. The GitOps path and branch must already exist before apply. For a private repository, provision a read-only Argo repository credential through the approved secret-delivery process; never pass repository credentials through this module or commit them to either branch.

The bootstrap `Application` enables prune and self-heal but forbids namespace creation. GitOps therefore owns every child declared below the supplied path, while OpenTofu retains only the Argo namespace, Helm release, and root bootstrap object rendered by that release.

Run `tofu test` in this directory. The mock test asserts that the chart is pinned, the server has no public service/ingress, admin login is disabled, and the root source is fixed to `gitops`.

# GitHub repository policy module

Manages policy for an existing repository. It does not create the repository, any branch, the protected orphan `gitops` tree, workflow files, variables, secrets, or credentials.

Three source rulesets enforce the branch ladder's merge mechanics: development requires one approval and squash, staging requires one approval and merge commits, and main requires two approvals and merge commits. Every branch dismisses stale reviews, requires code-owner and last-push approval, resolves conversations, forbids deletion and force pushes, has no administrator bypass, and requires the configured status checks. The `Validate trusted branch ladder` check is what constrains source branches (`feature/* -> development -> staging -> main`, plus the documented hotfix/back-merge exceptions).

The GitOps ruleset restricts updates and grants only the deployment GitHub App a pull-request-scoped bypass. Direct App pushes do not bypass the ruleset. Deployment PRs require both branch/GitOps policy checks; staging lock changes require one team approval and production lock changes require two. Development remains eligible for policy-controlled automatic merge. Path-specific reviewers use GitHub's ruleset required-reviewers capability and must be available for the repository's organization/plan before apply.

Workflow environments match the repository workflows. Plan environments carry no secret gate. Release, promotion, rollback, and apply environments use exact source-branch policies; every promotion, rollback, and apply environment requires the configured organization team, prevents self-review, and forbids administrator bypass.

Before the first apply, create the organization team and deployment App, install the App on the existing repository, configure exact status-check names, and verify the GitHub plan supports rulesets, required reviewers, and environment protection. Import any existing rulesets/environments rather than recreating them. Create the orphan `gitops` branch separately through the reviewed bootstrap runbook; its contents and credentials must never enter OpenTofu.

Run `tofu test` in this directory. The mock test covers approval counts, merge methods, non-bypassable source rules, App-only GitOps updates, path approvals, and protected apply environments.

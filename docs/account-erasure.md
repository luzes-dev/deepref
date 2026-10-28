# Account erasure and physical completion

The thirty-day cancellation window is unchanged. `accounts complete-deletions`
claims a request only after that window, scrubs the account and solely owned
workspace metadata, and attempts a bounded batch of due object deletions. Shared
workspaces and scientific actor identifiers remain. This does not promise that
an outage or delayed operator run can meet a physical deadline.

Logical anonymization and physical fulfillment have separate results. A request
stays `completing` while any targeted object, staging writer or unresolved upload
remains. Only actual successful deletion/NotFound responses, stopped writer-owned
staging cleanup and a resolved upload outcome permit a new fulfillment record.
The command reports `logically_anonymized`, `physically_pending`, actual storage
acknowledgements and `physical_erasure` counts. `accounts erasure-status` reports
pending/overdue objects, retry attempts, pending/overdue staging, unresolved
uploads and unverified historical fulfillment rows. Due requests and pending
account progress are counted even if workspace/export locks have not yet allowed
object targeting to commit. A historical NULL
`physical_verified_at` is not evidence that bytes remain or that they were
physically deleted.

Due privacy targets bypass the ordinary additional 24-hour object cleanup grace
after their metadata and retaining references have been withdrawn. They never
bypass the thirty days. Retirement commits before storage deletion and prevents
reattachment. Storage errors retain an overdue obligation and retry after five
minutes. An incomplete batch or failed run is not fulfillment or legal equivalence.

Workspace erasure cancels exports containing its bytes, including other users'
historical attempts whose latest request scope has changed. It targets only the
workspace's objects and archives containing that workspace; unrelated shared
workspace sources and newer unrelated archives remain. Source references are
released under the workspace/export/registry locks. Object acquisition and each
HTTP download chunk recheck live ownership, the exact artifact, cancellation,
retention and retirement; the existing absolute five-minute response deadline
also bounds those read locks. Bytes previously delivered to a recipient are
outside server erasure scope.

## Rollout

Keep old binaries stopped throughout activation. First drain older export and
deletion writers, remove their untracked private staging on every affected host,
and prevent restart. Old account objects without an attempt journal must receive
actual deletion acknowledgements too: mutable latest-attempt pointers cannot
establish the contents of every old archive. Existing export expiry and object
cleanup can retire and delete those objects; the activation function refuses
while any remain unacknowledged. An operator assertion does not perform this work.

After those actions, use the maintenance credential for:

```sh
deepref-server accounts activate-erasure --confirm-drained all-legacy-export-writers-stopped-and-untracked-staging-removed-no-restart
```

The default-false gate permits runtime reads but no runtime mutation. Its narrow
activation function is maintenance-only. Writer journals and per-account progress
participate in runtime business transactions. Role adoption and canonical-function
verification must include these exact definitions before deployment; migration
ownership remains separate from runtime. New unverified fulfillment inserts are
rejected after activation. Historical records are not relabelled.

## Interrupted writers and unknown uploads

Each export attempt durably records its scope and actual private staging path
before scientific writes. Its blocking owner keeps polling an outstanding
adapter operation after caller cancellation or deadline, removes its staging
after the operation stops, and only then records its stopped acknowledgement.
Caller timeouts, expired leases and dropped futures are never cleanup evidence.

A process crash or unlink failure leaves the writer pending. Establish which
host and exact owner held it, stop that owner and prevent restart before any
host-local recovery. Never remove another active writer's directory or infer
remote absence from a missing local path. This source provides no time-based
command that clears these obligations or manufactures a writer acknowledgement.

The storage adapter attempts multipart abort on failures but does not expose a
definite abort receipt. Such attempts remain `unresolved`, including after a
later object DELETE/NotFound. Provider multipart/late-completion reconciliation
and verified host recovery require separate evidence; neither elapsed time nor
an operator's unsupported assertion permits physical fulfillment. These cases
remain visible and need operator investigation, not a success flag.

## Restore and retention

Keep pending `account_erasure_progress` and its target maps with the restore
records as well as the five-year fulfillment log. Progress retains keyed address
digests until physical completion. Replay preserves the original logical scrub
cutoff, so a later physical acknowledgement cannot erase invitations for a new
registration using the same address. Historical proofs use their old completion
time as the fallback cutoff. Restore replay rechecks physical targets; external
identity/billing provider closure remains a separate obligation.

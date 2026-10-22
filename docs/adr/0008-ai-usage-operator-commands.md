# ADR 0008: AI usage recovery and limit changes use explicit operator commands

AI usage recovery runs as a workspace-scoped durable worker job so it can settle reservations after a worker crash. `deepref-server ai-usage reconcile` remains a read-only report for operations acceptance; separate `waive-overage` and `set-cap` commands make audited operator decisions without coupling a billing provider to the request path.

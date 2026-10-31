-- phase: expand
-- Append-only record of operator replays of terminal deliveries (quarantined
-- outbox events and dead-lettered inbox events). request_id makes a retried
-- operator command idempotent: the second run returns the recorded outcome.
CREATE TABLE public.delivery_recovery_audit (
    request_id uuid PRIMARY KEY,
    occurred_at timestamptz NOT NULL DEFAULT now(),
    operator_user_id uuid NOT NULL,
    delivery_kind text NOT NULL CHECK (delivery_kind IN ('outbox', 'inbox')),
    delivery_ref text NOT NULL CHECK (char_length(delivery_ref) BETWEEN 1 AND 64),
    outcome text NOT NULL CHECK (outcome IN ('replayed', 'not_quarantined')),
    reason text NOT NULL CHECK (char_length(btrim(reason)) BETWEEN 1 AND 500)
);

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'deepref_runtime') THEN
    REVOKE UPDATE, DELETE, TRUNCATE ON public.delivery_recovery_audit FROM deepref_runtime;
  END IF;
END
$$;

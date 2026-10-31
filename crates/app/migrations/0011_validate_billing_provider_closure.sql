-- phase: expand
-- Validate the provider-closure shape added NOT VALID in 0010. Existing rows
-- have NULL closure columns, which the shape allows.
ALTER TABLE public.billing_accounts
  VALIDATE CONSTRAINT billing_accounts_provider_closure_shape;

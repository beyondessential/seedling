-- Whether the last answer Canopy gave for a hostname rejected the runtime's
-- certificate signing request, which no later answer has replaced. It is told
-- apart from a failed order or a refusal by nothing else the row holds.
ALTER TABLE tls_chain_state ADD COLUMN csr_rejected INTEGER NOT NULL DEFAULT 0;

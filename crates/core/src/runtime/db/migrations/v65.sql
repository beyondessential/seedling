-- r[impl tls.csr.flow]
-- A CSR requests a set of hostnames rather than one.
--
-- The set is held as a JSON array of strings, in the order the operator gave
-- it. Every row with a requested hostname carries it over as a set of one, so
-- nothing the runtime already knew about a request is lost.
ALTER TABLE tls_certificates ADD COLUMN requested_hostnames TEXT;

UPDATE tls_certificates
SET requested_hostnames = json_array(requested_hostname)
WHERE requested_hostname IS NOT NULL;

ALTER TABLE tls_certificates DROP COLUMN requested_hostname;

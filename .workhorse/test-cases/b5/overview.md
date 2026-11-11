# Certificate SAN index

Scenarios for resolving certificates through the SAN index and for the sweeps that repair it.

## Resolution

- [x] A hostname no stored certificate names has no candidates and is not served, however many certificates are stored (verifies spec: tls.cert.serve)
- [x] A wildcard certificate is found for a name one label below it, ignoring case, and not for the bare domain or two labels down (verifies spec: tls.cert.validation.san-coverage)
- [x] A superseded certificate stays in the index but is not served
- [x] An index row naming a hostname the certificate's PEM does not carry is rewritten from the PEM, and the next candidate is served
- [x] The control-plane snapshot passes over an index row the PEM contradicts, like the serving lookup does
- [x] The snapshot finds an active certificate missing from the index
- [x] The snapshot and the serving lookup both rank by self-issuance read from the certificate, not the stored column
- [x] Index keys agree with SAN coverage for exact, wildcard and malformed names (verifies spec: tls.cert.validation.san-coverage)

## Writes and migration

- [x] Deleting a certificate removes its index entry
- [x] Storing a certificate whose PEM will not parse is refused and leaves nothing stored
- [x] The migration indexes every stored certificate, indexes an unparseable one as covering nothing, and skips rows with no PEM

## Sweeps

- [x] A PEM changed without reindexing leaves the certificate unindexed, and the unindexed sweep reindexes it
- [x] SAN rows deleted by hand are missed by the unindexed sweep and restored by the full sweep
- [x] A sweep leaves alone a certificate whose PEM changed after the sweep read it
- [ ] On a live daemon, a certificate uploaded through the operator interface is served at the next handshake, and the logs show no SAN index repairs

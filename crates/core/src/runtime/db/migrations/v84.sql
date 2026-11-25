-- The packet-filter fault was filed as `nftables_failed` before it took the
-- name of the facility rather than the Linux tool. Only `packet_filter_failed`
-- is cleared now, so an active fault under the old name would stay on the
-- fault list for good; the rows take the new name, and the next apply files or
-- clears it as usual.
UPDATE faults
SET kind = 'packet_filter_failed'
WHERE app = '_system'
  AND kind = 'nftables_failed';

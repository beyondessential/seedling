-- Resource types were recorded as their Rust names lowercased, so an external
-- volume was `externalvolume` and an external service `externalservice`; the
-- wire names them `external_volume` and `external_service`.
UPDATE stopped_resources SET kind = 'external_volume' WHERE kind = 'externalvolume';
UPDATE stopped_resources SET kind = 'external_service' WHERE kind = 'externalservice';
UPDATE faults SET resource_type = 'external_volume' WHERE resource_type = 'externalvolume';
UPDATE faults SET resource_type = 'external_service' WHERE resource_type = 'externalservice';
UPDATE instance_restarts SET resource_type = 'external_volume' WHERE resource_type = 'externalvolume';
UPDATE instance_restarts SET resource_type = 'external_service' WHERE resource_type = 'externalservice';
